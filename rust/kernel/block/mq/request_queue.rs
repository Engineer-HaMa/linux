// SPDX-License-Identifier: GPL-2.0

use super::Operations;
use crate::{
    bindings,
    error::{
        from_err_ptr,
        Error,
        Result,
    },
    sync::Arc,
    types::{
        ForeignOwnable,
        Opaque,
    },
};
use core::marker::PhantomData;

/// A structure describing the queues associated with a block device.
///
/// Owned by a [`GenDisk`].
///
/// # Invariants
///
/// - `self.0` is a valid `bindings::request_queue`.
/// - `self.0.queuedata` is a valid `T::QueueData`.
#[repr(transparent)]
pub struct RequestQueue<T>(Opaque<bindings::request_queue>, PhantomData<T>);

impl<T> RequestQueue<T>
where
    T: Operations,
{
    /// Create a [`RequestQueue`] from a raw `bindings::request_queue` pointer
    ///
    /// # Safety
    ///
    /// - `ptr` must be valid for use as a reference for the duration of `'a`.
    /// - `ptr` must have been initialized as part of [`GenDiskBuilder::build`].
    pub(crate) unsafe fn from_raw<'a>(ptr: *const bindings::request_queue) -> &'a Self {
        // INVARIANT:
        // - By function safety requirements, `ptr` is a valid `request_queue`.
        // - By function safety requirement `ptr` was initialized by [`GenDiskBuilder::build`], and
        //   thus `queuedata` was set to point to a valid `T::QueueData`.
        //
        // SAFETY: By function safety requirements `ptr` is valid for use as a reference.
        unsafe { &*ptr.cast() }
    }

    /// Get the driver private data associated with this [`RequestQueue`].
    pub fn queue_data(&self) -> <T::QueueData as ForeignOwnable>::Borrowed<'_> {
        // SAFETY: By type invariant, `queuedata` is a valid `T::QueueData`.
        unsafe { T::QueueData::borrow((*self.0.get()).queuedata) }
    }

    /// Stop all hardware queues of this [`RequestQueue`].
    pub fn stop_hw_queues(&self) {
        // SAFETY: By type invariant, `self.0` is a valid `request_queue`.
        unsafe { bindings::blk_mq_stop_hw_queues(self.0.get()) }
    }

    /// Start all hardware queues of this [`RequestQueue`].
    ///
    /// This function will mark the queues as ready and if necessary, schedule the queues to run.
    pub fn start_stopped_hw_queues_async(&self) {
        // SAFETY: By type invariant, `self.0` is a valid `request_queue`.
        unsafe { bindings::blk_mq_start_stopped_hw_queues(self.0.get(), true) }
    }
}

/// An owned `struct request_queue` allocated via `blk_mq_alloc_queue`.
///
/// Used for admin/sync request submission outside of the normal blk-mq path.
pub struct OwnedRequestQueue<T: Operations> {
    ptr: *mut bindings::request_queue,
    // Kept for ownership: ensures the tag set outlives the queue.
    _tagset: Arc<super::TagSet<T>>,
}

impl<T: Operations> OwnedRequestQueue<T> {
    /// Allocate a new request queue backed by the given tag set.
    pub fn try_new(tagset: Arc<super::TagSet<T>>, queue_data: T::QueueData) -> Result<Self> {
        // SAFETY: `tagset.raw_tag_set()` is valid for the duration of this call.
        let mq = from_err_ptr(unsafe {
            bindings::blk_mq_alloc_queue(
                tagset.raw_tag_set(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        })?;
        // SAFETY: `mq` is a valid `request_queue` returned by `blk_mq_alloc_queue`.
        unsafe { (*mq).queuedata = queue_data.into_foreign().cast() };
        Ok(Self {
            ptr: mq,
            _tagset: tagset,
        })
    }

    /// Allocate a synchronous request from this queue.
    pub fn alloc_sync_request(&self, op: u32) -> Result<SyncRequest<T>> {
        // SAFETY: `self.ptr` is a valid `request_queue` allocated by `blk_mq_alloc_queue`.
        let rq = from_err_ptr(unsafe { bindings::blk_mq_alloc_request(self.ptr, op, 0) })?;
        // SAFETY: `rq` is valid and ownership is transferred to `SyncRequest`.
        Ok(unsafe { SyncRequest::from_ptr(rq) })
    }
}

impl<T: Operations> Drop for OwnedRequestQueue<T> {
    fn drop(&mut self) {
        // TODO: Free queue, unless it has been adopted by a disk.
    }
}

/// A synchronous request to be submitted to a queue.
pub struct SyncRequest<T: Operations> {
    ptr: *mut bindings::request,
    _p: PhantomData<T>,
}

impl<T: Operations> SyncRequest<T> {
    /// Creates a new synchronous request from a raw pointer.
    ///
    /// # Safety
    ///
    /// `ptr` must be valid and ownership is transferred to the new `SyncRequest`.
    pub(crate) unsafe fn from_ptr(ptr: *mut bindings::request) -> Self {
        Self {
            ptr,
            _p: PhantomData,
        }
    }

    /// Submits the request for execution.
    pub fn execute(&self, at_head: bool) -> Result {
        // SAFETY: `self.ptr` is a valid request pointer owned by this `SyncRequest`.
        let status = unsafe { bindings::blk_execute_rq(self.ptr, at_head) };
        // SAFETY: `blk_status_to_errno` is always safe to call with any blk_status.
        let ret = unsafe { bindings::blk_status_to_errno(status) };
        if ret < 0 {
            Err(Error::from_errno(ret))
        } else {
            Ok(())
        }
    }

    /// Returns the tag associated with this synchronous request.
    pub fn tag(&self) -> i32 {
        // SAFETY: `self.ptr` is a valid request pointer owned by this `SyncRequest`.
        unsafe { (*self.ptr).tag }
    }

    /// Returns the per-request data associated with this synchronous request.
    pub fn data(&self) -> &T::RequestData {
        // SAFETY: `self.ptr` is valid. `blk_mq_rq_to_pdu` returns a pointer to the PDU
        // which was initialized as `T::RequestData` by the tag set initializer.
        unsafe { &*(bindings::blk_mq_rq_to_pdu(self.ptr).cast::<T::RequestData>()) }
    }
}

impl<T: Operations> Drop for SyncRequest<T> {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` is a valid request that was allocated via `blk_mq_alloc_request`.
        unsafe { bindings::blk_mq_free_request(self.ptr) };
    }
}
