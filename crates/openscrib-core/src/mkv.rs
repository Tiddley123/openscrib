//! Minimal streaming Matroska writer for one H.264 video and one Opus audio
//! track.
//!
//! Crash-safe by construction: the Segment and every Cluster are written with
//! "unknown" size, so a file cut off mid-recording still plays up to the last
//! flushed cluster. The only back-patch is `Duration`, written on `finish`.
//! Block timestamps are milliseconds on the recording clock, so a step's
//! `t_ms` is also its position in the video.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io::{self, Seek, SeekFrom, Write};

const EBML: u32 = 0x1A45_DFA3;
const EBML_VERSION: u32 = 0x4286;
const EBML_READ_VERSION: u32 = 0x42F7;
const EBML_MAX_ID_LENGTH: u32 = 0x42F2;
const EBML_MAX_SIZE_LENGTH: u32 = 0x42F3;
const DOC_TYPE: u32 = 0x4282;
const DOC_TYPE_VERSION: u32 = 0x4287;
const DOC_TYPE_READ_VERSION: u32 = 0x4285;
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TIMESTAMP_SCALE: u32 = 0x2A_D7B1;
const MUXING_APP: u32 = 0x4D80;
const WRITING_APP: u32 = 0x5741;
const DURATION: u32 = 0x4489;
const TRACKS: u32 = 0x1654_AE6B;
const TRACK_ENTRY: u32 = 0xAE;
const TRACK_NUMBER: u32 = 0xD7;
const TRACK_UID: u32 = 0x73C5;
const TRACK_TYPE: u32 = 0x83;
const FLAG_LACING: u32 = 0x9C;
const CODEC_ID: u32 = 0x86;
const CODEC_PRIVATE: u32 = 0x63A2;
const CODEC_DELAY: u32 = 0x56AA;
const SEEK_PRE_ROLL: u32 = 0x56BB;
const VIDEO: u32 = 0xE0;
const PIXEL_WIDTH: u32 = 0xB0;
const PIXEL_HEIGHT: u32 = 0xBA;
const AUDIO: u32 = 0xE1;
const SAMPLING_FREQUENCY: u32 = 0xB5;
const CHANNELS: u32 = 0x9F;
const CLUSTER: u32 = 0x1F43_B675;
const CLUSTER_TIMESTAMP: u32 = 0xE7;
const SIMPLE_BLOCK: u32 = 0xA3;

const UNKNOWN_SIZE: [u8; 8] = [0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

/// A cluster's block offsets are i16 ms; start a new one well before that.
const MAX_CLUSTER_MS: u64 = 30_000;
/// Start a new cluster at a keyframe once the current one is this old, so
/// seeking lands near the target.
const KEYFRAME_CLUSTER_MS: u64 = 5_000;

pub const OPUS_SAMPLE_RATE: u32 = 48_000;
/// libopus encoder lookahead at 48 kHz.
pub const OPUS_PRE_SKIP: u16 = 312;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Track {
    Video = 1,
    Audio = 2,
}

#[derive(Debug, Clone)]
pub struct VideoSpec {
    pub width: u32,
    pub height: u32,
    /// `AVCDecoderConfigurationRecord`, see [`avcc`].
    pub avcc: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct AudioSpec {
    pub channels: u8,
}

pub struct MkvWriter<W: Write + Seek> {
    w: W,
    duration_pos: u64,
    cluster_ts: Option<u64>,
    has_video: bool,
    has_audio: bool,
}

impl<W: Write + Seek> MkvWriter<W> {
    pub fn new(mut w: W, video: Option<&VideoSpec>, audio: Option<&AudioSpec>, app: &str) -> io::Result<Self> {
        let mut head = Vec::new();
        master(&mut head, EBML, |b| {
            uint(b, EBML_VERSION, 1);
            uint(b, EBML_READ_VERSION, 1);
            uint(b, EBML_MAX_ID_LENGTH, 4);
            uint(b, EBML_MAX_SIZE_LENGTH, 8);
            string(b, DOC_TYPE, "matroska");
            uint(b, DOC_TYPE_VERSION, 4);
            uint(b, DOC_TYPE_READ_VERSION, 2);
        });
        id(&mut head, SEGMENT);
        head.extend_from_slice(&UNKNOWN_SIZE);

        // Info, with Duration last so its offset is easy to find.
        let mut info = Vec::new();
        uint(&mut info, TIMESTAMP_SCALE, 1_000_000);
        string(&mut info, MUXING_APP, app);
        string(&mut info, WRITING_APP, app);
        id(&mut info, DURATION);
        size(&mut info, 8);
        let duration_in_info = info.len();
        info.extend_from_slice(&0f64.to_be_bytes());
        id(&mut head, INFO);
        size(&mut head, info.len() as u64);
        let duration_pos = w.stream_position()? + head.len() as u64 + duration_in_info as u64;
        head.extend_from_slice(&info);

        master(&mut head, TRACKS, |b| {
            if let Some(v) = video {
                master(b, TRACK_ENTRY, |b| {
                    uint(b, TRACK_NUMBER, Track::Video as u64);
                    uint(b, TRACK_UID, 0x5C2E_0001);
                    uint(b, TRACK_TYPE, 1);
                    uint(b, FLAG_LACING, 0);
                    string(b, CODEC_ID, "V_MPEG4/ISO/AVC");
                    bytes(b, CODEC_PRIVATE, &v.avcc);
                    master(b, VIDEO, |b| {
                        uint(b, PIXEL_WIDTH, v.width as u64);
                        uint(b, PIXEL_HEIGHT, v.height as u64);
                    });
                });
            }
            if let Some(a) = audio {
                master(b, TRACK_ENTRY, |b| {
                    uint(b, TRACK_NUMBER, Track::Audio as u64);
                    uint(b, TRACK_UID, 0x5C2E_0002);
                    uint(b, TRACK_TYPE, 2);
                    uint(b, FLAG_LACING, 0);
                    string(b, CODEC_ID, "A_OPUS");
                    bytes(b, CODEC_PRIVATE, &opus_head(a.channels));
                    uint(b, CODEC_DELAY, OPUS_PRE_SKIP as u64 * 1_000_000_000 / OPUS_SAMPLE_RATE as u64);
                    uint(b, SEEK_PRE_ROLL, 80_000_000);
                    master(b, AUDIO, |b| {
                        float(b, SAMPLING_FREQUENCY, OPUS_SAMPLE_RATE as f64);
                        uint(b, CHANNELS, a.channels as u64);
                    });
                });
            }
        });

        w.write_all(&head)?;
        w.flush()?;
        Ok(Self { w, duration_pos, cluster_ts: None, has_video: video.is_some(), has_audio: audio.is_some() })
    }

    /// Append one frame or packet. Timestamps should be roughly increasing
    /// across tracks; use [`Interleaver`] when two threads feed the writer.
    pub fn write(&mut self, track: Track, t_ms: u64, keyframe: bool, data: &[u8]) -> io::Result<()> {
        let ok = match track {
            Track::Video => self.has_video,
            Track::Audio => self.has_audio,
        };
        if !ok {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("no {track:?} track")));
        }

        let new_cluster = match self.cluster_ts {
            None => true,
            Some(c) => {
                t_ms < c
                    || t_ms - c >= MAX_CLUSTER_MS
                    || (keyframe && track == Track::Video && t_ms - c >= KEYFRAME_CLUSTER_MS)
            }
        };
        if new_cluster {
            // Flush what we have so a crash keeps every closed cluster.
            self.w.flush()?;
            let mut c = Vec::new();
            id(&mut c, CLUSTER);
            c.extend_from_slice(&UNKNOWN_SIZE);
            uint(&mut c, CLUSTER_TIMESTAMP, t_ms);
            self.w.write_all(&c)?;
            self.cluster_ts = Some(t_ms);
        }

        let rel = (t_ms - self.cluster_ts.unwrap_or(t_ms)) as i16;
        let mut hdr = Vec::with_capacity(16);
        id(&mut hdr, SIMPLE_BLOCK);
        size(&mut hdr, 4 + data.len() as u64);
        hdr.push(0x80 | track as u8);
        hdr.extend_from_slice(&rel.to_be_bytes());
        // Audio packets are all independently decodable for our purposes.
        hdr.push(if keyframe || track == Track::Audio { 0x80 } else { 0x00 });
        self.w.write_all(&hdr)?;
        self.w.write_all(data)
    }

    /// Patch `Duration` and hand back the writer.
    pub fn finish(mut self, duration_ms: u64) -> io::Result<W> {
        let end = self.w.stream_position()?;
        self.w.seek(SeekFrom::Start(self.duration_pos))?;
        self.w.write_all(&(duration_ms as f64).to_be_bytes())?;
        self.w.seek(SeekFrom::Start(end))?;
        self.w.flush()?;
        Ok(self.w)
    }
}

/// Orders packets from several producers by timestamp before they reach the
/// writer. A packet is released once a later packet is `window_ms` ahead of
/// it, so producers may lag each other by up to that much.
/// (timestamp, arrival order, track, keyframe, data); min-heap by the first two.
type Queued = Reverse<(u64, u64, Track, bool, Vec<u8>)>;

pub struct Interleaver {
    window_ms: u64,
    seq: u64,
    newest: u64,
    heap: BinaryHeap<Queued>,
}

impl Interleaver {
    pub fn new(window_ms: u64) -> Self {
        Self { window_ms, seq: 0, newest: 0, heap: BinaryHeap::new() }
    }

    pub fn push<W: Write + Seek>(
        &mut self,
        out: &mut MkvWriter<W>,
        track: Track,
        t_ms: u64,
        keyframe: bool,
        data: Vec<u8>,
    ) -> io::Result<()> {
        self.seq += 1;
        self.newest = self.newest.max(t_ms);
        self.heap.push(Reverse((t_ms, self.seq, track, keyframe, data)));
        self.drain(out, self.newest.saturating_sub(self.window_ms))
    }

    pub fn flush<W: Write + Seek>(&mut self, out: &mut MkvWriter<W>) -> io::Result<()> {
        self.drain(out, u64::MAX)
    }

    fn drain<W: Write + Seek>(&mut self, out: &mut MkvWriter<W>, upto: u64) -> io::Result<()> {
        while self.heap.peek().is_some_and(|Reverse((t, ..))| *t <= upto) {
            let Reverse((t, _, track, key, data)) = self.heap.pop().expect("peeked");
            out.write(track, t, key, &data)?;
        }
        Ok(())
    }
}

/// Split an H.264 Annex B stream into NAL units, without start codes.
pub fn annexb_nals(stream: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= stream.len() {
        if stream[i] == 0 && stream[i + 1] == 0 && stream[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut nals = Vec::with_capacity(starts.len());
    for (n, &s) in starts.iter().enumerate() {
        let mut end = starts.get(n + 1).map_or(stream.len(), |next| next - 3);
        // A 4-byte start code leaves a trailing zero on the previous NAL.
        while end > s && n + 1 < starts.len() && stream[end - 1] == 0 {
            end -= 1;
        }
        if end > s {
            nals.push(&stream[s..end]);
        }
    }
    nals
}

pub const NAL_IDR: u8 = 5;
pub const NAL_SPS: u8 = 7;
pub const NAL_PPS: u8 = 8;

pub fn nal_type(nal: &[u8]) -> u8 {
    nal.first().map_or(0, |b| b & 0x1F)
}

/// Convert Annex B to the 4-byte length-prefixed form Matroska stores.
pub fn to_length_prefixed(nals: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nals.iter().map(|n| n.len() + 4).sum());
    for n in nals {
        out.extend_from_slice(&(n.len() as u32).to_be_bytes());
        out.extend_from_slice(n);
    }
    out
}

/// Build an `AVCDecoderConfigurationRecord` from one SPS and one PPS.
pub fn avcc(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let mut b = vec![1, sps[1], sps[2], sps[3], 0xFF, 0xE1];
    b.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    b.extend_from_slice(sps);
    b.push(1);
    b.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    b.extend_from_slice(pps);
    b
}

fn opus_head(channels: u8) -> Vec<u8> {
    let mut b = b"OpusHead".to_vec();
    b.push(1);
    b.push(channels);
    b.extend_from_slice(&OPUS_PRE_SKIP.to_le_bytes());
    b.extend_from_slice(&OPUS_SAMPLE_RATE.to_le_bytes());
    b.extend_from_slice(&0i16.to_le_bytes());
    b.push(0);
    b
}

// --- EBML primitives ---

fn id(b: &mut Vec<u8>, id: u32) {
    let bytes = id.to_be_bytes();
    let skip = bytes.iter().take_while(|&&x| x == 0).count();
    b.extend_from_slice(&bytes[skip..]);
}

fn size(b: &mut Vec<u8>, n: u64) {
    // Smallest vint that fits; the all-ones value of each length is reserved.
    let len = (1..=8).find(|&l| n < (1u64 << (7 * l)) - 1).expect("size fits in 8 bytes");
    let marked = n | (1u64 << (7 * len));
    b.extend_from_slice(&marked.to_be_bytes()[8 - len..]);
}

fn uint(b: &mut Vec<u8>, eid: u32, v: u64) {
    let bytes = v.to_be_bytes();
    let skip = bytes.iter().take_while(|&&x| x == 0).count().min(7);
    id(b, eid);
    size(b, (8 - skip) as u64);
    b.extend_from_slice(&bytes[skip..]);
}

fn float(b: &mut Vec<u8>, eid: u32, v: f64) {
    id(b, eid);
    size(b, 8);
    b.extend_from_slice(&v.to_be_bytes());
}

fn string(b: &mut Vec<u8>, eid: u32, s: &str) {
    bytes(b, eid, s.as_bytes());
}

fn bytes(b: &mut Vec<u8>, eid: u32, data: &[u8]) {
    id(b, eid);
    size(b, data.len() as u64);
    b.extend_from_slice(data);
}

fn master(b: &mut Vec<u8>, eid: u32, body: impl FnOnce(&mut Vec<u8>)) {
    let mut inner = Vec::new();
    body(&mut inner);
    id(b, eid);
    size(b, inner.len() as u64);
    b.extend_from_slice(&inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use matroska_demuxer::{Frame, MatroskaFile, TrackType};
    use std::io::Cursor;

    // Baseline-profile SPS/PPS shaped like OpenH264's output.
    const SPS: &[u8] = &[0x67, 0x42, 0xC0, 0x1F, 0x8C, 0x8D, 0x40];
    const PPS: &[u8] = &[0x68, 0xCE, 0x3C, 0x80];

    #[test]
    fn vint_sizes() {
        let enc = |n| {
            let mut b = Vec::new();
            size(&mut b, n);
            b
        };
        assert_eq!(enc(0), [0x80]);
        assert_eq!(enc(126), [0xFE]);
        assert_eq!(enc(127), [0x40, 0x7F]);
        assert_eq!(enc(16382), [0x7F, 0xFE]);
        assert_eq!(enc(16383), [0x20, 0x3F, 0xFF]);
    }

    #[test]
    fn splits_annexb() {
        let stream = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 9];
        let nals = annexb_nals(&stream);
        assert_eq!(nals, [&[0x67, 1, 2][..], &[0x68, 3], &[0x65, 9, 9]]);
        assert_eq!(nal_type(nals[2]), NAL_IDR);
        assert_eq!(to_length_prefixed(&nals[1..2]), [0, 0, 0, 2, 0x68, 3]);
    }

    #[test]
    fn demuxer_reads_our_output() {
        let v = VideoSpec { width: 1920, height: 1080, avcc: avcc(SPS, PPS) };
        let a = AudioSpec { channels: 1 };
        let mut mkv = MkvWriter::new(Cursor::new(Vec::new()), Some(&v), Some(&a), "openscrib-test").unwrap();
        let mut il = Interleaver::new(1000);

        // Audio every 20 ms, video at 0, 7000 (keyframe) and 40000 ms, pushed
        // out of order the way two threads would.
        for t in (0..41_000).step_by(20) {
            il.push(&mut mkv, Track::Audio, t, true, vec![0xFC, t as u8]).unwrap();
            if t == 200 {
                il.push(&mut mkv, Track::Video, 0, true, vec![0, 0, 0, 1, 0x65]).unwrap();
            }
            if t == 7300 {
                il.push(&mut mkv, Track::Video, 7000, true, vec![0, 0, 0, 1, 0x65]).unwrap();
            }
            if t == 40_100 {
                il.push(&mut mkv, Track::Video, 40_000, false, vec![0, 0, 0, 1, 0x41]).unwrap();
            }
        }
        il.flush(&mut mkv).unwrap();
        let bytes = mkv.finish(41_000).unwrap().into_inner();

        let mut f = MatroskaFile::open(Cursor::new(bytes)).unwrap();
        assert_eq!(f.ebml_header().doc_type(), "matroska");
        assert_eq!(f.info().duration(), Some(41_000.0));
        assert_eq!(f.tracks().len(), 2);
        let vt = &f.tracks()[0];
        assert_eq!(vt.track_type(), TrackType::Video);
        assert_eq!(vt.codec_private(), Some(&avcc(SPS, PPS)[..]));
        assert_eq!(vt.video().unwrap().pixel_width().get(), 1920);
        let at = &f.tracks()[1];
        assert_eq!(at.codec_id(), "A_OPUS");
        assert_eq!(&at.codec_private().unwrap()[..8], b"OpusHead");

        let mut frame = Frame::default();
        let mut video_ts = Vec::new();
        let mut audio = 0;
        let mut last = 0;
        while f.next_frame(&mut frame).unwrap() {
            assert!(frame.timestamp >= last, "blocks out of order");
            last = frame.timestamp;
            match frame.track {
                1 => video_ts.push((frame.timestamp, frame.is_keyframe)),
                2 => audio += 1,
                _ => panic!("unknown track"),
            }
        }
        assert_eq!(video_ts, [(0, Some(true)), (7000, Some(true)), (40_000, Some(false))]);
        assert_eq!(audio, 2050);
    }

    #[test]
    fn truncated_file_still_parses() {
        let v = VideoSpec { width: 64, height: 64, avcc: avcc(SPS, PPS) };
        let mut mkv = MkvWriter::new(Cursor::new(Vec::new()), Some(&v), None, "t").unwrap();
        for t in (0..20_000).step_by(1000) {
            mkv.write(Track::Video, t, t % 6000 == 0, &[0, 0, 0, 1, 0x65]).unwrap();
        }
        // Simulate a crash: no finish(), and the last block is cut in half.
        let mut bytes = mkv.w.into_inner();
        bytes.truncate(bytes.len() - 3);
        let mut f = MatroskaFile::open(Cursor::new(bytes)).unwrap();
        let mut frame = Frame::default();
        let mut n = 0;
        while let Ok(true) = f.next_frame(&mut frame) {
            n += 1;
        }
        assert!(n >= 18, "only {n} frames survived");
    }
}
