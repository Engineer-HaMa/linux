use super::Operations;
use super::tag_set::TagSet;
use crate::{
    bindings,
    error::{from_err_ptr, Error, Result},
    sync::Arc,
    types::{ForeignOwnable, Opaque},
};
use core::marker::PhantomData;

#[repr(transparent)]
#[allow(missing_docs)]
pub struct RequestQueueRef<T>(Opaque<bindings::request_queue>, PhantomData<T>);

impl<T> RequestQueueRef<T>
where
    T: Operations,
{
    pub(crate) unsafe fn from_raw<'a>(ptr: *const bindings::request_queue) -> &'a Self {
        unsafe { &*ptr.cast() }
    }

    #[allow(missing_docs)]
    pub fn queue_data(&self) -> <T::QueueData as ForeignOwnable>::Borrowed<'_> {
        unsafe { T::QueueData::borrow((*self.0.get()).queuedata) }
    }

    #[allow(missing_docs)]
    pub fn stop_hw_queues(&self) {
        unsafe { bindings::blk_mq_stop_hw_queues(self.0.get()) }
    }

    #[allow(missing_docs)]
    pub fn start_stopped_hw_queues_async(&self) {
        unsafe { bindings::blk_mq_start_stopped_hw_queues(self.0.get(), true) }
    }
}

/// An owning request queue, used by drivers to allocate and manage their own queue.
pub struct RequestQueue<T: Operations> {
    ptr: *mut bindings::request_queue,
    _tagset: Arc<TagSet<T>>,
}

impl<T: Operations> RequestQueue<T> {
    /// Create a new request queue for the given tag set.
    pub fn try_new(tagset: Arc<TagSet<T>>, queue_data: T::QueueData) -> Result<Self> {
        let mq = from_err_ptr(unsafe {
            bindings::blk_mq_alloc_queue(
                tagset.raw_tag_set(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        })?;
        unsafe { (*mq).queuedata = queue_data.into_foreign() as _ };
        Ok(Self { ptr: mq, _tagset: tagset })
    }

    /// Allocate a synchronous request from this queue.
    pub fn alloc_sync_request(&self, op: u32) -> Result<SyncRequest<T>> {
        let rq = from_err_ptr(unsafe { bindings::blk_mq_alloc_request(self.ptr, op, 0) })?;
        Ok(unsafe { SyncRequest::from_ptr(rq) })
    }
}

impl<T: Operations> Drop for RequestQueue<T> {
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
    unsafe fn from_ptr(ptr: *mut bindings::request) -> Self {
        Self { ptr, _p: PhantomData }
    }

    /// Submits the request for execution.
    pub fn execute(&self, at_head: bool) -> Result {
        let status = unsafe { bindings::blk_execute_rq(self.ptr, at_head as _) };
        let ret = unsafe { bindings::blk_status_to_errno(status) };
        if ret < 0 {
            Err(Error::from_errno(ret))
        } else {
            Ok(())
        }
    }

    /// Returns the tag associated with this synchronous request.
    pub fn tag(&self) -> i32 {
        unsafe { (*self.ptr).tag }
    }

    /// Returns the per-request data associated with this synchronous request.
    pub fn data(&self) -> &T::RequestData {
        unsafe { &*(bindings::blk_mq_rq_to_pdu(self.ptr) as *const T::RequestData) }
    }
}

impl<T: Operations> Drop for SyncRequest<T> {
    fn drop(&mut self) {
        unsafe { bindings::blk_mq_free_request(self.ptr) };
    }
}
