use super::Operations;
use crate::types::{ForeignOwnable, Opaque};
use core::marker::PhantomData;

#[repr(transparent)]
#[allow(missing_docs)]
pub struct RequestQueue<T>(Opaque<bindings::request_queue>, PhantomData<T>);

impl<T> RequestQueue<T>
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
