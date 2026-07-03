// SPDX-License-Identifier: GPL-2.0

//! DMA mapping of a block request's data via `blk_rq_dma_map`, exposed as an
//! owned RAII mapping plus a fallible segment iterator.
//!
//! C header: [`include/linux/blk-mq-dma.h`](srctree/include/linux/blk-mq-dma.h)

use core::iter::FusedIterator;

use crate::{
    bindings,
    block::mq::{
        Operations,
        Request, //
    },
    device::Device,
    error::{
        code::EIO,
        Result, //
    },
    types::Opaque, //
};

impl<T: Operations> Request<T> {
    /// DMA-map this request's data on `dev`. `total_len` is the request payload
    /// length (also the unmap length). Peer-to-peer / MMIO mappings are rejected
    /// with `EIO`. See [`RequestDmaMapping`].
    pub fn dma_map(&self, dev: &Device, total_len: u32) -> Result<RequestDmaMapping> {
        RequestDmaMapping::new(self, dev, total_len)
    }
}

/// One DMA-mapped physical segment.
#[derive(Clone, Copy)]
pub struct DmaSegment {
    /// DMA address the device should use for this segment.
    pub addr: u64,
    /// Length of this segment in bytes.
    pub len: u32,
}

enum SegPos {
    First,
    Running,
    Done,
}

/// An owned DMA mapping of a request's data.
///
/// Unmaps on drop: storing it keeps the mapping alive, and dropping it (in the
/// completion path, or on an early error as rollback) tears it down. Iterate the
/// mapped segments with [`segments`](Self::segments).
///
/// # Invariants
///
/// While `live` is `true`, `req` points to the mapped request (which stays alive
/// at least until this mapping is dropped) and `dev` points to a device that
/// outlives the mapping.
pub struct RequestDmaMapping {
    req: *mut bindings::request,
    dev: *mut bindings::device,
    state: Opaque<bindings::dma_iova_state>,
    iter: Opaque<bindings::blk_dma_iter>,
    total_len: u32,
    pos: SegPos,
    live: bool,
}

impl RequestDmaMapping {
    fn new<T: Operations>(req: &Request<T>, dev: &Device, total_len: u32) -> Result<Self> {
        let mut m = Self {
            req: req.as_raw(),
            dev: dev.as_raw(),
            state: Opaque::zeroed(),
            iter: Opaque::zeroed(),
            total_len,
            pos: SegPos::First,
            live: false,
        };
        // SAFETY: `req`/`dev` are valid (live borrows); `state`/`iter` are zeroed
        // out-params the C call fully initializes.
        if !unsafe {
            bindings::blk_rq_dma_map_iter_start(m.req, m.dev, m.state.get(), m.iter.get())
        } {
            return Err(EIO);
        }
        // SAFETY: `iter` was initialized above.
        let map = unsafe { (*m.iter.get()).p2pdma.map };
        if map != bindings::pci_p2pdma_map_type_PCI_P2PDMA_MAP_NONE {
            // SAFETY: `iter` was initialized by the successful map above.
            let len = unsafe { (*m.iter.get()).len } as usize;
            // SAFETY: a mapping was established; tear it down with its actual type.
            unsafe { bindings::blk_rq_dma_unmap(m.req, m.dev, m.state.get(), len, map) };
            return Err(EIO);
        }
        // INVARIANT: `req`/`dev` are valid and the mapping is live.
        m.live = true;
        Ok(m)
    }

    /// A fallible iterator over the mapped segments.
    pub fn segments(&mut self) -> Segments<'_> {
        Segments { m: self }
    }

    fn current(&self) -> DmaSegment {
        // SAFETY: `iter` was initialized by `new` / `_next`.
        unsafe {
            DmaSegment {
                addr: (*self.iter.get()).addr,
                len: (*self.iter.get()).len,
            }
        }
    }
}

impl Drop for RequestDmaMapping {
    fn drop(&mut self) {
        if !self.live {
            return;
        }
        // SAFETY: by the type invariant `req`/`dev` are valid while `live`;
        // `state` was populated by iter_start; the mapping is `MAP_NONE`.
        let unmapped = unsafe {
            bindings::blk_rq_dma_unmap(
                self.req,
                self.dev,
                self.state.get(),
                self.total_len as usize,
                bindings::pci_p2pdma_map_type_PCI_P2PDMA_MAP_NONE,
            )
        };
        if !unmapped {
            crate::pr_warn!(
                "blk_rq_dma_unmap: manual per-segment unmap required but unsupported\n"
            );
        }
    }
}

/// Fallible iterator over a [`RequestDmaMapping`]'s segments. Advancing performs
/// the incremental DMA mapping (which can fail), so `Item` is a [`Result`].
pub struct Segments<'a> {
    m: &'a mut RequestDmaMapping,
}

impl Iterator for Segments<'_> {
    type Item = Result<DmaSegment>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.m.pos {
            SegPos::Done => None,
            SegPos::First => {
                self.m.pos = SegPos::Running;
                Some(Ok(self.m.current()))
            }
            SegPos::Running => {
                // SAFETY: by the mapping's type invariant `req`/`dev` are valid;
                // `iter` is initialized. Maps the next segment.
                let mapped = unsafe {
                    bindings::blk_rq_dma_map_iter_next(self.m.req, self.m.dev, self.m.iter.get())
                };
                if mapped {
                    return Some(Ok(self.m.current()));
                }
                self.m.pos = SegPos::Done;
                // SAFETY: `iter` initialized; `status` valid on `false`.
                if unsafe { (*self.m.iter.get()).status } == 0 {
                    None
                } else {
                    Some(Err(EIO))
                }
            }
        }
    }
}

impl FusedIterator for Segments<'_> {}
