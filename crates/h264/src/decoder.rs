//! Top-level decoder: NAL dispatch, picture boundaries, POC, DPB and output.
//!
//! The calling thread parses headers and does all DPB / reference list bookkeeping (which only needs
//! header information). Macroblock decoding of each picture runs as a job (on a thread pool with the
//! `threads` feature) that publishes finished macroblock rows into the picture's shared [`Frame`];
//! jobs of later pictures block per row on the reference data they need, so several pictures decode
//! concurrently (frame-level parallelism).

use crate::dpb::{Dpb, Output, OutputMeta, RefMark};
use crate::error::{Error, Result, ensure, invalid, unsupported};
use crate::params::{Pps, Sps};
use crate::picture::{Frame, FrameRef, MbKind, MbState, Planes, RefPic, Sample, stores_u8};
use crate::slice::{NalHeader, Poc, PocState, SliceHeader, SliceType, is_new_picture, nal_type};
use crate::slicedec::{PicState, SliceDecoder};
use crate::transform::LevelScale;
use crate::{ColorInfo, Picture};
use filmcraft_bitstream::{BitReader, annexb_nals, length_prefixed_nals, unescape_rbsp};
use std::collections::VecDeque;
#[cfg(feature = "threads")]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// The decoder supports at most 16 reference pictures, so constructing more
/// inferred gap pictures cannot improve a usable reference list.
const MAX_INFERRED_GAP_FRAMES: u32 = 16;

/// One slice of a picture job.
struct SliceJob {
    sh: SliceHeader,
    pps: Arc<Pps>,
    sps: Arc<Sps>,
    rbsp: Vec<u8>,
    refs: [Vec<RefPic>; 2],
    ls: Arc<LevelScale>,
}

/// A picture whose slices are being collected.
struct PendingPic {
    frame: FrameRef,
    sps: Arc<Sps>,
    first: SliceHeader,
    poc: Poc,
    pts: i64,
    key: bool,
    slices: Vec<SliceJob>,
}

/// Counters describing what the decoder has seen (useful to check test coverage).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DecodeStats {
    pub pictures: u64,
    /// Pictures reconstructed as independent slice jobs.
    pub slice_parallel_pictures: u64,
    pub slices_cavlc: u64,
    pub slices_cabac: u64,
    pub slices_i: u64,
    pub slices_p: u64,
    pub slices_b: u64,
    pub mb_i4x4: u64,
    pub mb_i8x8: u64,
    pub mb_i16x16: u64,
    pub mb_pcm: u64,
    pub mb_p_skip: u64,
    pub mb_b_skip: u64,
    pub mb_b_direct16x16: u64,
    pub mb_inter: u64,
    /// Inter macroblocks using the 8x8 transform.
    pub mb_inter_8x8_transform: u64,
    pub mmco_ops: u64,
    pub long_term_marks: u64,
    pub frame_num_gaps: u64,
    pub weighted_slices: u64,
    pub temporal_direct_slices: u64,
    /// Non-reference pictures decoded in draft mode (deblocking skipped).
    pub draft_pictures: u64,
}

/// State shared with decoding jobs.
struct Shared {
    error: Mutex<Option<Error>>,
    stats: Mutex<DecodeStats>,
    /// Recycled picture buffers.
    pool: Arc<Mutex<Vec<PicState<u8>>>>,
    pool16: Arc<Mutex<Vec<PicState<u16>>>>,
    #[cfg(feature = "threads")]
    sub_pool: Arc<Mutex<Vec<PicState<u8>>>>,
    #[cfg(feature = "threads")]
    sub_pool16: Arc<Mutex<Vec<PicState<u16>>>>,
    pool_cap: usize,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            error: Mutex::new(None),
            stats: Mutex::new(DecodeStats::default()),
            pool: Arc::default(),
            pool16: Arc::default(),
            pool_cap: 4,
            #[cfg(feature = "threads")]
            sub_pool: Arc::default(),
            #[cfg(feature = "threads")]
            sub_pool16: Arc::default(),
        }
    }
}

#[derive(Default)]
struct PicturePools {
    pool: Arc<Mutex<Vec<PicState<u8>>>>,
    pool16: Arc<Mutex<Vec<PicState<u16>>>>,
    #[cfg(feature = "threads")]
    sub_pool: Arc<Mutex<Vec<PicState<u8>>>>,
    #[cfg(feature = "threads")]
    sub_pool16: Arc<Mutex<Vec<PicState<u16>>>>,
}

static PICTURE_POOLS: OnceLock<PicturePools> = OnceLock::new();
#[cfg(all(feature = "threads", not(target_arch = "wasm32")))]
static WORKER_POOL: OnceLock<Option<Arc<rayon::ThreadPool>>> = OnceLock::new();

/// H.264 decoder.
pub struct Decoder {
    spss: Vec<Option<Arc<Sps>>>,
    ppss: Vec<Option<Arc<Pps>>>,
    nal_length_size: Option<usize>,
    dpb: Dpb<FrameRef>,
    poc_state: PocState,
    prev_ref_frame_num: u32,
    pending: Option<PendingPic>,
    next_id: u32,
    active_sps: Option<Arc<Sps>>,
    ls_cache: Vec<(Arc<Pps>, Arc<LevelScale>)>,
    /// Pictures leaving the DPB, converted once their decoding job has finished.
    out_queue: VecDeque<Output<FrameRef>>,
    shared: Arc<Shared>,
    #[cfg(feature = "threads")]
    pool: Option<Arc<rayon::ThreadPool>>,
    in_flight: VecDeque<FrameRef>,
    max_in_flight: usize,
    threads: usize,
    /// Draft mode: skip deblocking of non-reference pictures ([`Decoder::set_draft`]).
    draft: bool,
    intra_only: bool,
    slice_parallel: bool,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

/// The worker threads [`Decoder::new`] uses: the available cores (at most 16) with the `threads`
/// feature on native targets, 1 otherwise.
pub fn default_threads() -> usize {
    #[cfg(all(feature = "threads", not(target_arch = "wasm32")))]
    {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(16)
    }
    #[cfg(not(all(feature = "threads", not(target_arch = "wasm32"))))]
    {
        1
    }
}

/// Whether an Annex B access unit proves an intra-profile stream with IDR slices only.
pub fn stream_is_intra_annexb(data: &[u8]) -> bool {
    std::panic::catch_unwind(|| intra_annexb(data)).unwrap_or(false)
}

fn intra_annexb(data: &[u8]) -> bool {
    let mut dec = Decoder::with_threads(1);
    let mut slices = false;
    for nal in annexb_nals(data) {
        let Some((&header, rbsp)) = nal.split_first() else { return false };
        let Ok(hdr) = NalHeader::parse(header) else { return false };
        match hdr.nal_unit_type {
            nal_type::SPS => {
                let Ok(sps) = Sps::parse(&unescape_rbsp(rbsp)) else { return false };
                if !sps.is_intra_profile() || dec.handle_nal(nal, 0).is_err() {
                    return false;
                }
            }
            nal_type::PPS => {
                if dec.handle_nal(nal, 0).is_err() {
                    return false;
                }
            }
            nal_type::IDR => {
                let rbsp = unescape_rbsp(rbsp);
                let parsed = SliceHeader::parse(&rbsp, hdr, |id| {
                    let pps = dec.ppss.get(id as usize).and_then(Option::as_ref).ok_or_else(|| Error::Invalid("missing PPS".into()))?;
                    let sps = dec.spss.get(pps.sps_id as usize).and_then(Option::as_ref).ok_or_else(|| Error::Invalid("missing SPS".into()))?;
                    Ok((&**pps, &**sps))
                });
                if parsed.is_err() {
                    return false;
                }
                slices = true;
            }
            nal_type::SLICE | nal_type::SLICE_DPA | nal_type::SLICE_DPB | nal_type::SLICE_DPC => return false,
            _ => {}
        }
    }
    slices && dec.intra_profile_configured()
}

impl Decoder {
    /// A decoder using all available cores (with the `threads` feature).
    pub fn new() -> Self {
        Self::with_threads(default_threads())
    }

    /// A decoder using up to `threads` worker threads (1 = decode on the calling thread).
    pub fn with_threads(threads: usize) -> Self {
        crate::cavlc::init_tables();
        #[cfg(feature = "threads")]
        let pool = if threads > 1 && cfg!(not(target_arch = "wasm32")) {
            rayon::ThreadPoolBuilder::new().num_threads(threads).thread_name(|i| format!("h264-{i}")).build().ok().map(Arc::new)
        } else {
            None
        };
        #[cfg(feature = "threads")]
        let workers = if pool.is_some() { threads } else { 1 };
        #[cfg(not(feature = "threads"))]
        let workers = 1;
        Decoder {
            spss: vec![None; 32],
            ppss: vec![None; 256],
            nal_length_size: None,
            dpb: Dpb::new(),
            poc_state: PocState::default(),
            prev_ref_frame_num: 0,
            pending: None,
            next_id: 1,
            active_sps: None,
            ls_cache: Vec::new(),
            out_queue: VecDeque::new(),
            shared: Arc::new(Shared::default()),
            #[cfg(feature = "threads")]
            pool,
            in_flight: VecDeque::new(),
            max_in_flight: threads.max(1) + 2,
            threads: workers,
            draft: false,
            intra_only: false,
            slice_parallel: true,
        }
    }

    /// Enable independent reconstruction of eligible intra slices (on by default).
    pub fn set_slice_parallel(&mut self, on: bool) {
        self.slice_parallel = on;
    }

    /// Whether independent reconstruction of eligible intra slices is enabled.
    pub fn slice_parallel(&self) -> bool {
        self.slice_parallel
    }

    /// A per-sample decoder sharing the process worker pool and recycled picture buffers.
    pub fn with_shared_pool() -> Self {
        let mut dec = Self::with_threads(1);
        let pools = PICTURE_POOLS.get_or_init(PicturePools::default);
        dec.shared = Arc::new(Shared {
            pool: pools.pool.clone(),
            pool16: pools.pool16.clone(),
            pool_cap: 6,
            #[cfg(feature = "threads")]
            sub_pool: pools.sub_pool.clone(),
            #[cfg(feature = "threads")]
            sub_pool16: pools.sub_pool16.clone(),
            ..Shared::default()
        });
        dec.max_in_flight = 2;
        #[cfg(all(feature = "threads", not(target_arch = "wasm32")))]
        {
            let pool = WORKER_POOL.get_or_init(|| {
                let threads = default_threads();
                if threads <= 1 {
                    return None;
                }
                rayon::ThreadPoolBuilder::new().num_threads(threads).thread_name(|i| format!("h264-shared-{i}")).build().ok().map(Arc::new)
            });
            if let Some(pool) = pool {
                dec.threads = pool.current_num_threads();
                dec.pool = Some(pool.clone());
            }
        }
        dec
    }

    /// Require IDR slices for independently decoded samples.
    pub fn set_intra_only(&mut self, on: bool) {
        self.intra_only = on;
    }

    /// Whether all configured sequence parameter sets promise intra pictures.
    pub fn intra_profile_configured(&self) -> bool {
        self.spss.iter().any(Option::is_some) && self.spss.iter().flatten().all(|s| s.is_intra_profile()) && self.ppss.iter().any(Option::is_some)
    }

    /// Worker threads decoding pictures (1: on the calling thread).
    pub fn threads(&self) -> usize {
        self.threads
    }

    /// Draft mode (off by default) for reduced-resolution playback: pictures submitted from now
    /// on that are non-reference pictures (`nal_ref_idc` 0) skip the deblocking filter (8.7).
    /// Nothing predicts from a non-reference picture (and deblocking never changes the motion
    /// data used for direct prediction), so every other picture stays bit-exact; the draft
    /// pictures themselves are approximate and flagged [`Picture::draft`]. Never use it for
    /// export or for a frame shown while paused.
    pub fn set_draft(&mut self, on: bool) {
        self.draft = on;
    }

    pub fn draft(&self) -> bool {
        self.draft
    }

    /// Configure from an `avcC` (AVCDecoderConfigurationRecord) box payload.
    pub fn from_avcc(avcc: &[u8]) -> Result<Self> {
        let mut d = Decoder::new();
        d.configure_avcc(avcc)?;
        Ok(d)
    }

    /// Parse an `avcC` record: SPS/PPS and the NAL length size used by [`Decoder::decode`].
    pub fn configure_avcc(&mut self, avcc: &[u8]) -> Result<()> {
        ensure!(avcc.len() >= 7, "avcC too short");
        ensure!(avcc[0] == 1, "unsupported avcC version {}", avcc[0]);
        self.nal_length_size = Some((avcc[4] & 3) as usize + 1);
        let mut pos = 5;
        let read_sets = |pos: &mut usize, count: usize, d: &mut Decoder| -> Result<()> {
            for _ in 0..count {
                ensure!(*pos + 2 <= avcc.len(), "avcC truncated");
                let len = u16::from_be_bytes([avcc[*pos], avcc[*pos + 1]]) as usize;
                *pos += 2;
                ensure!(*pos + len <= avcc.len(), "avcC truncated");
                d.handle_nal(&avcc[*pos..*pos + len], 0)?;
                *pos += len;
            }
            Ok(())
        };
        let nsps = (avcc[pos] & 0x1f) as usize;
        pos += 1;
        read_sets(&mut pos, nsps, self)?;
        ensure!(pos < avcc.len(), "avcC truncated");
        let npps = avcc[pos] as usize;
        pos += 1;
        read_sets(&mut pos, npps, self)
    }

    /// Length of NAL length prefixes (from avcC), or None for Annex-B input.
    pub fn nal_length_size(&self) -> Option<usize> {
        self.nal_length_size
    }

    /// Decode one access unit (all NAL units of one picture, Annex-B or length-prefixed per
    /// configuration; a byte stream with several complete access units is accepted too). Returns
    /// pictures that became ready for output, in output order; `pts` travels with the picture of
    /// this access unit through reordering.
    pub fn decode(&mut self, data: &[u8], pts: i64) -> Result<Vec<Picture>> {
        let nals = match self.nal_length_size {
            Some(n) => length_prefixed_nals(data, n)?,
            None => annexb_nals(data),
        };
        let mut result = Ok(());
        for nal in nals {
            if let Err(e) = self.handle_nal(nal, pts) {
                result = Err(e);
                break;
            }
        }
        // The access unit is complete: hand its picture to a decoding job.
        self.submit_pending()?;
        if let Some(e) = self.take_error() {
            result = result.and(Err(e));
        }
        result?;
        // Return finished pictures without blocking on frames still being decoded (bounded latency).
        let max_queue = self.max_in_flight * 2;
        let mut pics = Vec::new();
        while let Some(o) = self.out_queue.front() {
            if !o.frame.is_ready() && self.out_queue.len() <= max_queue {
                break;
            }
            let Some(o) = self.out_queue.pop_front() else { break };
            pics.push(make_picture(&o));
        }
        Ok(pics)
    }

    /// Output all remaining pictures (end of stream).
    pub fn flush(&mut self) -> Vec<Picture> {
        let _ = self.submit_pending();
        let mut outs = Vec::new();
        self.dpb.flush(&mut outs);
        self.emit(outs);
        let pics = self.out_queue.drain(..).map(|o| make_picture(&o)).collect();
        while let Some(f) = self.in_flight.pop_front() {
            f.wait_complete();
        }
        pics
    }

    /// Statistics accumulated so far (macroblock counts include finished pictures only).
    pub fn stats(&self) -> DecodeStats {
        self.shared.stats.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn stat(&self, f: impl FnOnce(&mut DecodeStats)) {
        if let Ok(mut s) = self.shared.stats.lock() {
            f(&mut s);
        }
    }

    /// First error reported by a decoding job since the last call (also returned by
    /// [`Decoder::decode`]); useful after [`Decoder::flush`].
    pub fn take_error(&mut self) -> Option<Error> {
        self.shared.error.lock().ok().and_then(|mut e| e.take())
    }

    fn handle_nal(&mut self, nal: &[u8], pts: i64) -> Result<()> {
        if nal.is_empty() {
            return Ok(());
        }
        let hdr = NalHeader::parse(nal[0])?;
        match hdr.nal_unit_type {
            nal_type::SPS => {
                let sps = Sps::parse(&unescape_rbsp(&nal[1..]))?;
                let id = sps.id as usize;
                self.spss[id] = Some(Arc::new(sps));
            }
            nal_type::PPS => {
                let rbsp = unescape_rbsp(&nal[1..]);
                let spss: Vec<Option<Sps>> = self.spss.iter().map(|s| s.as_ref().map(|s| (**s).clone())).collect();
                let pps = Pps::parse(&rbsp, &spss)?;
                let id = pps.id as usize;
                self.ppss[id] = Some(Arc::new(pps));
            }
            nal_type::SLICE | nal_type::IDR => self.handle_slice(nal, hdr, pts)?,
            nal_type::SLICE_DPA | nal_type::SLICE_DPB | nal_type::SLICE_DPC => {
                return unsupported("data partitioning (Extended profile)");
            }
            nal_type::END_SEQ | nal_type::END_STREAM => self.submit_pending()?,
            // SEI, AUD, filler, SPS extension, prefix NAL, subset SPS, auxiliary and extension slices: skipped.
            _ => {}
        }
        Ok(())
    }

    fn level_scale(&mut self, pps: &Arc<Pps>) -> Arc<LevelScale> {
        if let Some((_, ls)) = self.ls_cache.iter().find(|(p, _)| Arc::ptr_eq(p, pps)) {
            return ls.clone();
        }
        let ls: Arc<LevelScale> = Arc::from(LevelScale::new(&pps.scaling));
        self.ls_cache.retain(|(p, _)| self.ppss.iter().flatten().any(|q| Arc::ptr_eq(p, q)));
        self.ls_cache.push((pps.clone(), ls.clone()));
        ls
    }

    fn handle_slice(&mut self, nal: &[u8], hdr: NalHeader, pts: i64) -> Result<()> {
        let rbsp = unescape_rbsp(&nal[1..]);
        let ppss = &self.ppss;
        let spss = &self.spss;
        let mut found: Option<(Arc<Pps>, Arc<Sps>)> = None;
        let (sh, _, _) = SliceHeader::parse(&rbsp, hdr, |id| {
            let pps = ppss[id as usize].as_ref().ok_or_else(|| Error::MissingParameterSet(format!("PPS {id}")))?;
            let sps = spss[pps.sps_id as usize].as_ref().ok_or_else(|| Error::MissingParameterSet(format!("SPS {}", pps.sps_id)))?;
            found = Some((pps.clone(), sps.clone()));
            Ok((&**pps, &**sps))
        })?;
        if self.intra_only && !sh.idr {
            return invalid("non-IDR slice in an intra-only stream");
        }
        let Some((pps, sps)) = found else {
            return Err(Error::MissingParameterSet("PPS".into()));
        };
        sps.check_supported()?;
        if pps.num_slice_groups > 1 {
            return unsupported("slice groups (FMO)");
        }
        if sh.field_pic {
            return unsupported("field pictures");
        }
        if matches!(sh.slice_type, SliceType::Sp | SliceType::Si) {
            return unsupported("SP/SI slices");
        }
        if sh.redundant_pic_cnt > 0 {
            return Ok(()); // redundant slices are ignored
        }
        let new_pic = match &self.pending {
            None => true,
            Some(p) => is_new_picture(&p.first, &sh, &sps) || sh.first_mb_in_slice == 0 || !Arc::ptr_eq(&p.sps, &sps),
        };
        if new_pic {
            self.submit_pending()?;
            if sh.first_mb_in_slice != 0 && self.active_sps.is_none() {
                return invalid("stream does not start with the first slice of a picture");
            }
            self.start_picture(&sh, &sps, pts)?;
        }
        let ls = self.level_scale(&pps);
        let Some(pending) = self.pending.as_mut() else {
            return invalid("slice without a started picture");
        };
        let refs = self
            .dpb
            .build_ref_lists(&sh, pending.poc.frame(), sps.max_frame_num(), |e| RefPic { frame: e.frame.clone(), long_term: e.mark == RefMark::Long })?;
        if !sh.slice_type.is_intra() {
            ensure!(!refs[0].is_empty(), "no reference pictures available for inter slice");
            if sh.slice_type.is_b() {
                ensure!(!refs[1].is_empty(), "empty RefPicList1 in B slice");
            }
        }
        let weighted = (pps.weighted_pred && sh.slice_type.is_p()) || (pps.weighted_bipred_idc != 0 && sh.slice_type.is_b());
        let temporal = sh.slice_type.is_b() && !sh.direct_spatial_mv_pred;
        let (cabac, st, mmcos, lt) = (
            pps.entropy_coding_mode,
            sh.slice_type,
            sh.mmcos.len() as u64,
            sh.long_term_reference as u64 + sh.mmcos.iter().filter(|m| m.op == 3 || m.op == 6).count() as u64,
        );
        pending.slices.push(SliceJob { sh, pps, sps, rbsp, refs, ls });
        self.stat(|s| {
            if cabac {
                s.slices_cabac += 1;
            } else {
                s.slices_cavlc += 1;
            }
            match st {
                SliceType::I | SliceType::Si => s.slices_i += 1,
                SliceType::P | SliceType::Sp => s.slices_p += 1,
                SliceType::B => s.slices_b += 1,
            }
            s.weighted_slices += weighted as u64;
            s.temporal_direct_slices += temporal as u64;
            s.mmco_ops += mmcos;
            s.long_term_marks += lt;
        });
        Ok(())
    }

    fn start_picture(&mut self, sh: &SliceHeader, sps: &Arc<Sps>, pts: i64) -> Result<()> {
        if !sh.idr && let Some(active) = &self.active_sps {
            ensure!(
                active.bit_depth_luma == sps.bit_depth_luma && active.bit_depth_chroma == sps.bit_depth_chroma && active.chroma_format_idc == sps.chroma_format_idc,
                "bit depth or chroma format change requires an IDR picture"
            );
        }
        // Activate SPS; a resolution change flushes the DPB.
        let changed = match &self.active_sps {
            None => true,
            Some(a) => a.width() != sps.width() || a.height() != sps.height() || a.max_dpb_frames() != sps.max_dpb_frames() || a.chroma_format_idc != sps.chroma_format_idc,
        };
        if changed && self.active_sps.is_some() {
            let mut outs = Vec::new();
            self.dpb.flush(&mut outs);
            self.emit(outs);
            self.dpb.entries.clear();
        }
        self.active_sps = Some(sps.clone());
        self.dpb.capacity = sps.max_dpb_frames();
        self.dpb.max_reorder = sps.max_num_reorder_frames().min(self.dpb.capacity);
        let mb_w = sps.pic_width_in_mbs as usize;
        let mb_h = sps.frame_height_in_mbs() as usize;
        if sh.idr {
            let mut outs = Vec::new();
            self.dpb.idr(sh.no_output_of_prior_pics, &mut outs);
            self.emit(outs);
            self.prev_ref_frame_num = 0;
        } else {
            let max = sps.max_frame_num();
            if sh.frame_num != self.prev_ref_frame_num && sh.frame_num != (self.prev_ref_frame_num + 1) % max {
                let gap = (sh.frame_num + max - self.prev_ref_frame_num - 1) % max;
                if gap > MAX_INFERRED_GAP_FRAMES {
                    return Err(Error::Invalid("frame number gap exceeds the supported reference limit".into()));
                }
                // frame_num gap (8.2.5.2): insert "non-existing" frames. Their samples are never used by
                // conforming streams; copy the latest decoded frame for concealment.
                let meta = Arc::new(output_meta(sps, pts, false));
                let next_id = &mut self.next_id;
                let poc_state = &mut self.poc_state;
                let last = self.dpb.entries.iter().filter(|e| !e.non_existing).max_by_key(|e| e.frame.id).map(|e| e.frame.clone());
                let mut make = |_fnum: u32| {
                    let id = *next_id;
                    *next_id += 1;
                    let bit_depth = sps.bit_depth_luma;
                    let frame = if stores_u8(bit_depth, sps.bit_depth_chroma) {
                        gap_frame_as::<u8>(last.as_deref(), id, mb_w, mb_h, bit_depth, sps.chroma_format_idc as u8)
                    } else {
                        gap_frame_as::<u16>(last.as_deref(), id, mb_w, mb_h, bit_depth, sps.chroma_format_idc as u8)
                    };
                    (Arc::new(frame), 0, meta.clone())
                };
                let mut upd = |fnum: u32| poc_state.update_gap_frame(fnum, sps);
                self.shared.stats.lock().map(|mut s| s.frame_num_gaps += 1).ok();
                self.dpb.fill_frame_num_gap(self.prev_ref_frame_num, sh.frame_num, max, sps.max_num_ref_frames as usize, &mut make, &mut upd);
                self.prev_ref_frame_num = (sh.frame_num + max - 1) % max;
            }
        }
        let poc = self.poc_state.compute(sh, sps);
        // MMCO5: the picture's POC becomes relative to itself (tempPicOrderCnt, 8.2.1).
        let frame_poc = if sh.has_mmco5() { 0 } else { poc.frame() };
        let id = self.next_id;
        self.next_id += 1;
        let frame = Arc::new(Frame::new(id, frame_poc, mb_w, mb_h, sps.bit_depth_luma, sps.chroma_format_idc as u8));
        self.pending = Some(PendingPic { frame, sps: sps.clone(), first: sh.clone(), poc, pts, key: sh.idr, slices: Vec::new() });
        Ok(())
    }

    /// Start decoding the pending picture and do its reference marking / DPB insertion.
    fn submit_pending(&mut self) -> Result<()> {
        let Some(p) = self.pending.take() else { return Ok(()) };
        let PendingPic { frame, sps, first, poc, pts, key, slices } = p;
        let draft = self.draft && first.nal_ref_idc == 0;
        let (cx, cy, cw, ch) = sps.crop_rect();
        self.dispatch(frame.clone(), slices, draft, (cx as usize, cy as usize, cw as usize, ch as usize));
        self.poc_state.update(&first, &poc);
        if first.nal_ref_idc != 0 {
            self.prev_ref_frame_num = if first.has_mmco5() { 0 } else { first.frame_num };
        }
        let meta = Arc::new(OutputMeta { draft, ..output_meta(&sps, pts, key) });
        let mut outs = Vec::new();
        let fpoc = frame.poc;
        self.dpb.store_picture(&first, frame, fpoc, (sps.max_frame_num(), sps.max_num_ref_frames as usize), meta, &mut outs)?;
        self.emit(outs);
        Ok(())
    }

    fn dispatch(&mut self, frame: FrameRef, slices: Vec<SliceJob>, draft: bool, crop: (usize, usize, usize, usize)) {
        frame.expect_output();
        let shared = self.shared.clone();
        #[cfg(feature = "threads")]
        if let Some(pool) = &self.pool {
            self.in_flight.retain(|f| !f.is_published(f.mb_h() - 1));
            while self.in_flight.len() >= self.max_in_flight {
                if let Some(f) = self.in_flight.pop_front() {
                    f.wait_complete();
                }
            }
            self.in_flight.push_back(frame.clone());
            if self.slice_parallel {
                let starts: Vec<_> = slices.iter().map(|s| (s.sh.first_mb_in_slice as usize, s.sh.slice_type == SliceType::I)).collect();
                if let Some(ranges) = parallel_ranges(&starts, frame.mb_w, frame.mb_h()) {
                    spawn_parallel(pool, frame, slices, ranges, shared, (draft, crop));
                    return;
                }
            }
            pool.spawn_fifo(move || run_job(frame, slices, &shared, draft, crop));
            return;
        }
        run_job(frame, slices, &shared, draft, crop);
    }

    fn emit(&mut self, outs: Vec<Output<FrameRef>>) {
        self.out_queue.extend(outs);
    }
}

/// Whole-row intra slices can reconstruct independently of their absolute row.
#[cfg(any(feature = "threads", test))]
fn parallel_ranges(starts: &[(usize, bool)], mb_w: usize, mb_h: usize) -> Option<Vec<(usize, usize)>> {
    let total = mb_w.checked_mul(mb_h)?;
    if mb_w == 0 || starts.len() < 2 || starts.first()?.0 != 0 {
        return None;
    }
    let mut ranges = Vec::with_capacity(starts.len());
    for (k, &(first, is_i)) in starts.iter().enumerate() {
        let end = starts.get(k + 1).map_or(total, |s| s.0);
        if !is_i || first >= total || !first.is_multiple_of(mb_w) || end <= first || end > total || !end.is_multiple_of(mb_w) {
            return None;
        }
        ranges.push((first, (end - first) / mb_w));
    }
    Some(ranges)
}

#[cfg(feature = "threads")]
enum SliceOutcome<S: Sample> {
    // Keep result slots small while each picture owns its reconstruction buffers.
    Done { pic: Box<PicState<S>>, error: Option<Error> },
    Panicked,
}

#[cfg(feature = "threads")]
struct ParJob<S: Sample> {
    frame: FrameRef,
    slices: Vec<SliceJob>,
    ranges: Vec<(usize, usize)>,
    results: Mutex<Vec<Option<SliceOutcome<S>>>>,
    remaining: AtomicUsize,
    draft: bool,
    crop: (usize, usize, usize, usize),
    pool: Arc<Mutex<Vec<PicState<S>>>>,
    sub_pool: Arc<Mutex<Vec<PicState<S>>>>,
    shared: Arc<Shared>,
}

#[cfg(feature = "threads")]
fn spawn_parallel(
    pool: &rayon::ThreadPool,
    frame: FrameRef,
    slices: Vec<SliceJob>,
    ranges: Vec<(usize, usize)>,
    shared: Arc<Shared>,
    options: (bool, (usize, usize, usize, usize)),
) {
    if stores_u8(frame.bit_depth, frame.bit_depth) {
        let pools = SlicePools { pool: shared.pool.clone(), sub_pool: shared.sub_pool.clone() };
        ParJob::new(frame, slices, ranges, shared, options, pools).spawn(pool);
    } else {
        let pools = SlicePools { pool: shared.pool16.clone(), sub_pool: shared.sub_pool16.clone() };
        ParJob::new(frame, slices, ranges, shared, options, pools).spawn(pool);
    }
}

#[cfg(feature = "threads")]
struct SlicePools<S: Sample> {
    pool: Arc<Mutex<Vec<PicState<S>>>>,
    sub_pool: Arc<Mutex<Vec<PicState<S>>>>,
}

#[cfg(feature = "threads")]
impl<S: Sample> ParJob<S> {
    fn new(
        frame: FrameRef,
        slices: Vec<SliceJob>,
        ranges: Vec<(usize, usize)>,
        shared: Arc<Shared>,
        (draft, crop): (bool, (usize, usize, usize, usize)),
        pools: SlicePools<S>,
    ) -> Arc<Self> {
        let count = slices.len();
        Arc::new(Self {
            frame,
            slices,
            ranges,
            shared,
            draft,
            crop,
            pool: pools.pool,
            sub_pool: pools.sub_pool,
            results: Mutex::new((0..count).map(|_| None).collect()),
            remaining: AtomicUsize::new(count),
        })
    }

    fn spawn(self: Arc<Self>, pool: &rayon::ThreadPool) {
        for k in 0..self.slices.len() {
            let job = self.clone();
            pool.spawn_fifo(move || job.run_slice(k));
        }
    }

    fn run_slice(&self, k: usize) {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.reconstruct(k))).unwrap_or(SliceOutcome::Panicked);
        {
            let mut results = self.results.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(slot) = results.get_mut(k) {
                *slot = Some(outcome);
            }
        }
        // The mutex protects the result slots; the counter elects exactly one finisher.
        if self.remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.finish();
        }
    }

    fn reconstruct(&self, k: usize) -> SliceOutcome<S> {
        let (Some(slice), Some(&(_, rows))) = (self.slices.get(k), self.ranges.get(k)) else {
            return SliceOutcome::Panicked;
        };
        let frame = Arc::new(Frame::new(self.frame.id, self.frame.poc, self.frame.mb_w, rows, self.frame.bit_depth, self.frame.chroma_format_idc));
        let mut pic = recycled_picture(frame, &self.sub_pool);
        pic.recon_only = true;
        let mut header = slice.sh.clone();
        // Keep header_bits unchanged: the RBSP still contains the absolute address.
        header.first_mb_in_slice = 0;
        let error = decode_slice_header(slice, &header, &mut pic).err();
        SliceOutcome::Done { pic: Box::new(pic), error }
    }

    fn finish(&self) {
        let mut results = std::mem::take(&mut *self.results.lock().unwrap_or_else(PoisonError::into_inner));
        let mut main = recycled_picture(self.frame.clone(), &self.pool);
        main.skip_deblock = self.draft;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if results.iter().any(|r| !matches!(r, Some(SliceOutcome::Done { .. }))) {
                return Err(());
            }
            let mut first_error = None;
            for (k, (outcome, &(first_mb, _))) in results.iter_mut().zip(&self.ranges).enumerate() {
                if let Some(SliceOutcome::Done { pic, error }) = outcome {
                    main.adopt_slice(pic, first_mb, k as u32);
                    if first_error.is_none() {
                        first_error = error.take();
                    }
                }
            }
            main.flush_rows(true);
            Ok(first_error)
        }))
        .unwrap_or(Err(()));
        {
            let mut pool = self.sub_pool.lock().unwrap_or_else(PoisonError::into_inner);
            for outcome in results.into_iter().flatten() {
                if let SliceOutcome::Done { pic, .. } = outcome
                    && pool.len() < 16
                {
                    pool.push(*pic);
                }
            }
        }
        self.shared.stats.lock().unwrap_or_else(PoisonError::into_inner).slice_parallel_pictures += 1;
        finish_job(self.frame.clone(), main, &self.shared, JobEnv { draft: self.draft, crop: self.crop, pool: &self.pool }, result);
    }
}

/// Decode all slices of one picture and publish its rows.
fn run_job(frame: FrameRef, slices: Vec<SliceJob>, shared: &Shared, draft: bool, crop: (usize, usize, usize, usize)) {
    if stores_u8(frame.bit_depth, frame.bit_depth) {
        run_job_as::<u8>(frame, slices, shared, JobEnv { draft, crop, pool: &shared.pool });
    } else {
        run_job_as::<u16>(frame, slices, shared, JobEnv { draft, crop, pool: &shared.pool16 });
    }
}

struct JobEnv<'a, S: Sample> {
    draft: bool,
    crop: (usize, usize, usize, usize),
    pool: &'a Mutex<Vec<PicState<S>>>,
}

fn run_job_as<S: Sample>(frame: FrameRef, slices: Vec<SliceJob>, shared: &Shared, env: JobEnv<'_, S>) {
    let JobEnv { draft, crop, pool } = env;
    let mut pic = recycled_picture(frame.clone(), pool);
    pic.skip_deblock = draft;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut first_err = None;
        for s in &slices {
            if let Err(e) = decode_slice(s, &mut pic) {
                first_err.get_or_insert(e);
            }
        }
        pic.flush_rows(true);
        first_err
    }));
    finish_job(frame, pic, shared, JobEnv { draft, crop, pool }, result.map_err(|_| ()));
}

/// Reuse only buffers whose sample geometry matches the new frame.
fn recycled_picture<S: Sample>(frame: FrameRef, pool: &Mutex<Vec<PicState<S>>>) -> PicState<S> {
    let mut available = pool.lock().unwrap_or_else(PoisonError::into_inner);
    let index = available.iter().position(|p| {
        p.mb_w == frame.mb_w && p.mb_h == frame.mb_h() && p.frame.bit_depth == frame.bit_depth && p.frame.chroma_format_idc == frame.chroma_format_idc
    });
    let reuse = index.map(|i| available.swap_remove(i));
    drop(available);
    match reuse {
        Some(mut pic) => {
            pic.reset(frame);
            pic
        }
        None => PicState::new(frame),
    }
}

/// Both job paths publish fallback rows and signal completion in the same order.
fn finish_job<S: Sample>(frame: FrameRef, mut pic: PicState<S>, shared: &Shared, env: JobEnv<'_, S>, result: std::result::Result<Option<Error>, ()>) {
    let JobEnv { draft, crop, pool } = env;
    let mut err = match result {
        Ok(e) => e,
        Err(()) => {
            let fallback = Planes::<S>::gray(frame.width, frame.height, frame.bit_depth, frame.chroma_format_idc);
            let blank = vec![MbState::default(); frame.mb_w * frame.mb_h()];
            for r in 0..frame.mb_h() {
                if !frame.is_published(r) {
                    frame.publish(r, Frame::make_row(&fallback, r, &blank, &|_, _, _| u32::MAX));
                }
            }
            pic = PicState::new(frame.clone());
            Some(Error::Invalid("internal error while decoding a picture".into()))
        }
    };
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| frame.prepare_output(crop))).is_err() {
        err.get_or_insert_with(|| Error::Invalid("internal error while preparing a picture".into()));
    }
    if let Ok(mut s) = shared.stats.lock() {
        s.pictures += 1;
        s.draft_pictures += draft as u64;
        for st in &pic.mbs {
            match st.kind {
                MbKind::I4x4 => s.mb_i4x4 += 1,
                MbKind::I8x8 => s.mb_i8x8 += 1,
                MbKind::I16x16 => s.mb_i16x16 += 1,
                MbKind::IPcm => s.mb_pcm += 1,
                MbKind::PSkip => s.mb_p_skip += 1,
                MbKind::BSkip => s.mb_b_skip += 1,
                MbKind::BDirect16x16 => s.mb_b_direct16x16 += 1,
                MbKind::Inter => s.mb_inter += 1,
                MbKind::None => {}
            }
            if st.transform_8x8 && !st.kind.is_intra() {
                s.mb_inter_8x8_transform += 1;
            }
        }
    }
    if let (Some(e), Ok(mut slot)) = (err, shared.error.lock()) {
        slot.get_or_insert(e);
    }
    if let Ok(mut p) = pool.lock()
        && p.len() < shared.pool_cap
    {
        p.push(pic);
    }
    frame.finish_output();
}

fn decode_slice<S: Sample>(s: &SliceJob, pic: &mut PicState<S>) -> Result<()> {
    decode_slice_header(s, &s.sh, pic)
}

fn decode_slice_header<S: Sample>(s: &SliceJob, header: &SliceHeader, pic: &mut PicState<S>) -> Result<()> {
    let mut sd = SliceDecoder::new(header, &s.pps, &s.sps, pic, &s.refs, &s.ls)?;
    if s.pps.entropy_coding_mode {
        sd.decode_cabac(&s.rbsp)
    } else {
        let mut r = BitReader::new(&s.rbsp);
        r.seek_bits(header.header_bits);
        sd.decode_cavlc(&mut r)
    }
}

fn output_meta(sps: &Sps, pts: i64, key: bool) -> OutputMeta {
    let vui = sps.vui.clone().unwrap_or_default();
    OutputMeta {
        pts,
        key,
        crop: sps.crop_rect(),
        full_range: vui.full_range,
        colour_primaries: vui.colour_primaries,
        transfer_characteristics: vui.transfer_characteristics,
        matrix_coefficients: vui.matrix_coefficients,
        sar: vui.sar,
        draft: false,
    }
}

fn make_picture(o: &Output<FrameRef>) -> Picture {
    let (cx, cy, cw, ch) = o.meta.crop;
    let (cx, cy, cw, ch) = (cx as usize, cy as usize, cw as usize, ch as usize);
    let (y, u, v) = o.frame.take_output((cx, cy, cw, ch)).unwrap_or_else(|| o.frame.copy_cropped((cx, cy, cw, ch)));
    let (ccw, cch) = (cw.div_ceil(2), ch.div_ceil(16 / o.frame.mb_height_c));
    Picture {
        width: cw as u32,
        bit_depth: o.frame.bit_depth,
        height: ch as u32,
        chroma_format_idc: o.frame.chroma_format_idc,
        chroma_width: ccw as u32,
        chroma_height: cch as u32,
        y,
        u,
        v,
        y_stride: cw,
        uv_stride: ccw,
        pts: o.meta.pts,
        poc: o.poc,
        key: o.meta.key,
        color: ColorInfo {
            full_range: o.meta.full_range,
            primaries: o.meta.colour_primaries,
            transfer: o.meta.transfer_characteristics,
            matrix: o.meta.matrix_coefficients,
        },
        sar: o.meta.sar,
        draft: o.meta.draft,
    }
}

/// Samples for a non-existing reference picture (8.2.5.2).
fn gap_frame_as<S: Sample>(last: Option<&Frame>, id: u32, mb_w: usize, mb_h: usize, bit_depth: u32, chroma_format_idc: u8) -> Frame {
    let planes = match last {
        Some(f) if f.mb_w == mb_w && f.mb_h() == mb_h && f.bit_depth == bit_depth && f.chroma_format_idc == chroma_format_idc => {
            let mut p = Planes::<S>::new(mb_w * 16, mb_h * 16, bit_depth, chroma_format_idc);
            let (y, u, v) = f.copy_cropped((0, 0, mb_w * 16, mb_h * 16));
            if let (Some(y), Some(u), Some(v)) = (S::plane_samples(y), S::plane_samples(u), S::plane_samples(v)) {
                p.y = y;
                p.cb = u;
                p.cr = v;
            }
            p
        }
        _ => Planes::<S>::gray(mb_w * 16, mb_h * 16, bit_depth, chroma_format_idc),
    };
    Frame::from_planes(id, 0, &planes, bit_depth)
}

#[cfg(test)]
mod parallel_tests {
    use super::parallel_ranges;
    use crate::slice::SliceType;
    #[cfg(feature = "threads")]
    use super::{ParJob, PendingPic, Shared, SliceOutcome, SlicePools, decode_slice, spawn_parallel};
    #[cfg(feature = "threads")]
    use crate::picture::{Frame, MbKind};
    #[cfg(feature = "threads")]
    use crate::slicedec::PicState;
    #[cfg(feature = "threads")]
    use crate::{Decoder, Error};
    #[cfg(feature = "threads")]
    use filmcraft_bitstream::annexb_nals;
    #[cfg(feature = "threads")]
    use std::sync::Arc;

    #[test]
    fn parallel_ranges_one_slice() {
        assert_eq!(parallel_ranges(&[(0, true)], 4, 4), None);
        assert_eq!(parallel_ranges(&[], 4, 4), None);
    }

    #[test]
    fn parallel_ranges_two_row_bands() {
        assert_eq!(parallel_ranges(&[(0, true), (8, true)], 4, 4), Some(vec![(0, 2), (8, 2)]));
    }

    #[test]
    fn parallel_ranges_mid_row() {
        assert_eq!(parallel_ranges(&[(0, true), (5, true)], 4, 4), None);
    }

    #[test]
    fn parallel_ranges_out_of_order() {
        for starts in [&[(0, true), (8, true), (4, true)][..], &[(0, true), (4, true), (4, true)]] {
            assert_eq!(parallel_ranges(starts, 4, 4), None);
        }
    }

    #[test]
    fn parallel_ranges_p_slice() {
        for kind in [SliceType::P, SliceType::B, SliceType::Sp] {
            assert_eq!(parallel_ranges(&[(0, true), (4, kind == SliceType::I)], 4, 4), None);
        }
    }

    #[test]
    fn parallel_ranges_si_slice() {
        assert_eq!(parallel_ranges(&[(0, SliceType::Si == SliceType::I), (4, true)], 4, 4), None);
    }

    #[test]
    fn parallel_ranges_invalid_geometry() {
        assert_eq!(parallel_ranges(&[(4, true), (8, true)], 4, 4), None);
        assert_eq!(parallel_ranges(&[(0, true), (16, true)], 4, 4), None);
        assert_eq!(parallel_ranges(&[(0, true), (4, true)], 0, 4), None);
        assert_eq!(parallel_ranges(&[(0, true), (4, true)], 4, 0), None);
        assert_eq!(parallel_ranges(&[(0, true), (4, true)], usize::MAX, 4), None);
    }

    #[cfg(feature = "threads")]
    fn pcm_job(cabac: bool, bands: bool) -> PendingPic {
        let mut decoder = Decoder::with_threads(1);
        for nal in annexb_nals(&crate::synth_tests::pcm_row_bands(cabac, bands)) {
            decoder.handle_nal(nal, 0).unwrap();
        }
        decoder.pending.take().unwrap()
    }

    #[test]
    #[cfg(feature = "threads")]
    fn adopt_slice_pcm_matches_whole_picture() {
        let workers = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
        for cabac in [false, true] {
            let whole = pcm_job(cabac, false);
            let mut reference = PicState::<u8>::new(whole.frame);
            decode_slice(&whole.slices[0], &mut reference).unwrap();
            reference.flush_rows(true);
            // Only the slice identity changes when PCM data is split into row bands.
            for mb in reference.mbs.iter_mut().skip(reference.mb_w) {
                mb.slice_num = 1;
            }
            let bands = pcm_job(cabac, true);
            let shared = Arc::new(Shared::default());
            let starts: Vec<_> = bands.slices.iter().map(|s| (s.sh.first_mb_in_slice as usize, true)).collect();
            let ranges = parallel_ranges(&starts, bands.frame.mb_w, bands.frame.mb_h()).unwrap();
            let frame = bands.frame;
            let crop = (0, 0, frame.width, frame.height);
            frame.expect_output();
            spawn_parallel(&workers, frame.clone(), bands.slices, ranges, shared.clone(), (false, crop));
            let output = frame.take_output(crop).unwrap();
            assert_eq!(output.0.as_u8().unwrap(), reference.planes.y.as_slice());
            assert_eq!(output.1.as_u8().unwrap(), reference.planes.cb.as_slice());
            assert_eq!(output.2.as_u8().unwrap(), reference.planes.cr.as_slice());
            let main = shared.pool.lock().unwrap().pop().unwrap();
            assert_eq!(main.planes.y, reference.planes.y);
            assert_eq!(main.planes.cb, reference.planes.cb);
            assert_eq!(main.planes.cr, reference.planes.cr);
            assert_eq!(main.mbs, reference.mbs);
            assert_eq!(main.decoded_mbs, reference.decoded_mbs);
            assert_eq!(main.slices.len(), 2);
            assert_eq!(shared.sub_pool.lock().unwrap().len(), 2);
            assert_eq!(shared.stats.lock().unwrap().slice_parallel_pictures, 1);
        }
    }

    #[cfg(feature = "threads")]
    fn band_job() -> Arc<ParJob<u8>> {
        let bands = pcm_job(false, true);
        let starts: Vec<_> = bands.slices.iter().map(|s| (s.sh.first_mb_in_slice as usize, true)).collect();
        let ranges = parallel_ranges(&starts, bands.frame.mb_w, bands.frame.mb_h()).unwrap();
        let crop = (0, 0, bands.frame.width, bands.frame.height);
        bands.frame.expect_output();
        let shared = Arc::new(Shared::default());
        let pools = SlicePools { pool: shared.pool.clone(), sub_pool: shared.sub_pool.clone() };
        ParJob::new(bands.frame, bands.slices, ranges, shared, (false, crop), pools)
    }

    #[test]
    #[cfg(feature = "threads")]
    fn parallel_slice_panic_publishes_fallback_and_finishes() {
        let job = band_job();
        // A damaged recycled buffer forces an actual unwind in PCM reconstruction.
        let frame = Arc::new(Frame::new(1, 0, job.frame.mb_w, 1, 8, 1));
        let mut damaged = PicState::new(frame);
        damaged.planes.y.clear();
        job.sub_pool.lock().unwrap().push(damaged);
        job.run_slice(0);
        assert!(!job.frame.is_ready());
        assert!(matches!(job.results.lock().unwrap()[0], Some(SliceOutcome::Panicked)));
        job.run_slice(1);
        assert!(job.frame.is_ready());
        let (y, u, v) = job.frame.take_output(job.crop).unwrap();
        for plane in [y, u, v] {
            assert!(plane.to_le_bytes().iter().all(|&v| v == 128));
        }
        assert_eq!(job.shared.error.lock().unwrap().as_ref(), Some(&Error::Invalid("internal error while decoding a picture".into())));
        assert_eq!(job.shared.stats.lock().unwrap().pictures, 1);
        assert_eq!(job.shared.stats.lock().unwrap().slice_parallel_pictures, 1);
    }

    #[test]
    #[cfg(feature = "threads")]
    fn parallel_errors_follow_slice_order() {
        let mut job = band_job();
        let state = Arc::get_mut(&mut job).unwrap();
        // Distinct decoding errors, with the higher numbered slice finishing first.
        state.slices[0].sh.header_bits = 0;
        state.slices[0].rbsp.clear();
        state.slices[1].sh.header_bits = 0;
        state.slices[1].sh.slice_type = SliceType::P;
        state.slices[1].rbsp = vec![0x08, 0]; // mb_skip_run = 15, beyond the band
        let expected = match state.reconstruct(0) {
            SliceOutcome::Done { error: Some(error), .. } => error,
            _ => panic!("expected a decoding error"),
        };
        let later = match state.reconstruct(1) {
            SliceOutcome::Done { error: Some(error), .. } => error,
            _ => panic!("expected a second decoding error"),
        };
        assert_ne!(expected, later);
        job.run_slice(1);
        assert!(!job.frame.is_ready());
        job.run_slice(0);
        assert!(job.frame.is_ready());
        assert_eq!(job.shared.error.lock().unwrap().as_ref(), Some(&expected));
        assert!(job.frame.take_output(job.crop).is_some());
        assert_eq!(job.sub_pool.lock().unwrap().len(), 2);
    }

    #[test]
    #[cfg(feature = "threads")]
    fn parallel_assembly_panic_publishes_fallback_and_finishes() {
        let job = band_job();
        for k in 0..2 {
            job.results.lock().unwrap()[k] = Some(job.reconstruct(k));
        }
        // A damaged recycled destination forces an actual unwind in deblocking.
        let mut main = PicState::new(job.frame.clone());
        main.mbs.clear();
        job.pool.lock().unwrap().push(main);
        job.finish();
        assert!(job.frame.is_ready());
        assert_eq!(job.shared.error.lock().unwrap().as_ref(), Some(&Error::Invalid("internal error while decoding a picture".into())));
        assert!(job.frame.take_output(job.crop).is_some());
        assert_eq!(job.sub_pool.lock().unwrap().len(), 2);
    }

    #[test]
    #[cfg(feature = "threads")]
    fn parallel_draft_skips_whole_picture_deblocking() {
        let mut outputs = Vec::new();
        for draft in [false, true] {
            let mut job = band_job();
            Arc::get_mut(&mut job).unwrap().draft = draft;
            for k in 0..2 {
                let mut outcome = job.reconstruct(k);
                if let SliceOutcome::Done { pic, error } = &mut outcome {
                    assert!(error.is_none());
                    let pic = pic.as_mut();
                    for mb in &mut pic.mbs {
                        mb.kind = MbKind::I16x16;
                        mb.qp = 40;
                        mb.qpc = [37; 2];
                    }
                    for plane in [&mut pic.planes.y, &mut pic.planes.cb, &mut pic.planes.cr] {
                        plane.fill(100 + k as u8 * 3);
                    }
                }
                job.results.lock().unwrap()[k] = Some(outcome);
            }
            job.finish();
            assert!(job.frame.is_ready());
            assert!(job.shared.error.lock().unwrap().is_none());
            let main = job.pool.lock().unwrap().pop().unwrap();
            assert_eq!(main.skip_deblock, draft);
            outputs.push(job.frame.take_output(job.crop).unwrap());
        }
        assert_ne!(outputs[0], outputs[1]);
        let expected: Vec<u8> = (0..4 * 16 * 3 * 16).map(|i| if i < 4 * 16 * 16 { 100 } else { 103 }).collect();
        assert_eq!(outputs[1].0.as_u8().unwrap(), expected.as_slice());
    }
}
