//! Per-instance data of the instanced model draws: one vertex buffer of fixed capacity shared by all
//! batches, filled from what was pushed this frame and written at most once per frame.

use crate::gpu_types::InstanceRaw;

const INSTANCE_SIZE: usize = std::mem::size_of::<InstanceRaw>();

/// Instances the buffer holds unless the caller reserves another number.
pub(crate) const DEFAULT_CAPACITY: usize = 512;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Range {
    first: u32,
    count: u32,
}

#[derive(Default)]
pub(crate) struct InstanceStore {
    buffer: Option<wgpu::Buffer>,
    capacity: usize,
    /// This frame's instances in push order, each with its batch.
    pending: Vec<(u32, InstanceRaw)>,
    /// The same grouped by batch: what the buffer is about to hold.
    staged: Vec<InstanceRaw>,
    /// What the buffer holds; unchanged instances upload nothing.
    uploaded: Vec<InstanceRaw>,
    /// Per batch: where its instances are in `staged`.
    ranges: Vec<Range>,
    last_ranges: Vec<Range>,
}

impl InstanceStore {
    /// Make room for `capacity` instances per frame over all batches. Creates the buffer, so this belongs
    /// to loading; a smaller or equal request changes nothing.
    pub(crate) fn reserve(
        &mut self,
        device: &wgpu::Device,
        capacity: usize,
        counters: &mut crate::renderer::GpuCounters,
    ) {
        if self.buffer.is_some() && capacity <= self.capacity {
            return;
        }
        counters.buffers += 1;
        self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (capacity * INSTANCE_SIZE) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        self.capacity = capacity;
        // The new buffer holds nothing yet.
        self.uploaded.clear();
        // All the memory a frame needs, so pushing never allocates.
        for list in [&mut self.staged, &mut self.uploaded] {
            list.reserve(capacity.saturating_sub(list.len()));
        }
        self.pending
            .reserve(capacity.saturating_sub(self.pending.len()));
    }

    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }

    /// Register one more batch; returns its index.
    pub(crate) fn add_batch(&mut self) -> usize {
        self.ranges.push(Range::default());
        self.ranges.len() - 1
    }

    /// Forget every batch and what was pushed for them.
    pub(crate) fn clear(&mut self) {
        self.pending.clear();
        self.ranges.clear();
    }

    /// Drop what was pushed for a frame that is not drawn.
    pub(crate) fn discard(&mut self) {
        self.pending.clear();
    }

    /// Queue one instance for the next frame. False when the batch does not exist or the buffer is full.
    pub(crate) fn push(&mut self, batch: usize, raw: InstanceRaw) -> bool {
        if batch >= self.ranges.len() || self.pending.len() >= self.capacity {
            return false;
        }
        self.pending.push((batch as u32, raw));
        true
    }

    /// Group this frame's instances by batch and bring the buffer up to date. Returns the bytes uploaded
    /// and whether anything differs from the last frame.
    pub(crate) fn prepare(&mut self, queue: &wgpu::Queue) -> (usize, bool) {
        std::mem::swap(&mut self.ranges, &mut self.last_ranges);
        self.ranges.clear();
        self.ranges.resize(self.last_ranges.len(), Range::default());
        for (batch, _) in &self.pending {
            self.ranges[*batch as usize].count += 1;
        }
        // Counting sort: each batch gets a contiguous run, filled in push order.
        let mut first = 0;
        for r in &mut self.ranges {
            r.first = first;
            first += r.count;
            r.count = 0;
        }
        self.staged.clear();
        self.staged
            .resize(self.pending.len(), InstanceRaw::default());
        for (batch, raw) in self.pending.drain(..) {
            let r = &mut self.ranges[batch as usize];
            self.staged[(r.first + r.count) as usize] = raw;
            r.count += 1;
        }
        let mut uploaded = 0;
        if let Some(buffer) = &self.buffer {
            // A shorter list that is a prefix of what the buffer holds needs no upload either.
            if !self.staged.is_empty() && !self.uploaded.starts_with(&self.staged) {
                queue.write_buffer(buffer, 0, bytemuck::cast_slice(&self.staged));
                uploaded = self.staged.len() * INSTANCE_SIZE;
                std::mem::swap(&mut self.staged, &mut self.uploaded);
            }
        }
        (uploaded, uploaded > 0 || self.ranges != self.last_ranges)
    }

    pub(crate) fn buffer(&self) -> Option<&wgpu::Buffer> {
        self.buffer.as_ref()
    }

    /// Byte range and instance count of a batch in the buffer, as prepared for this frame.
    pub(crate) fn range(&self, batch: usize) -> (std::ops::Range<u64>, u32) {
        let r = self.ranges.get(batch).copied().unwrap_or_default();
        let start = r.first as u64 * INSTANCE_SIZE as u64;
        (
            start..start + r.count as u64 * INSTANCE_SIZE as u64,
            r.count,
        )
    }

    /// Instances prepared for this frame.
    pub(crate) fn count(&self) -> usize {
        self.ranges.iter().map(|r| r.count as usize).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(x: f32) -> InstanceRaw {
        InstanceRaw {
            rows: [[x; 4]; 3],
            color: [1.0; 4],
        }
    }

    #[test]
    fn pushes_need_a_batch_and_room() {
        let mut s = InstanceStore::default();
        assert!(!s.push(0, raw(1.0)), "no batch");
        let b = s.add_batch();
        assert!(!s.push(b, raw(1.0)), "no buffer, so no capacity");
        s.capacity = 2;
        assert!(s.push(b, raw(1.0)));
        assert!(s.push(b, raw(2.0)));
        assert!(!s.push(b, raw(3.0)), "full");
        s.discard();
        assert!(s.push(b, raw(3.0)));
        s.clear();
        assert!(!s.push(b, raw(4.0)), "batches are gone");
    }
}
