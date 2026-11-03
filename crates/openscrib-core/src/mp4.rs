//! Minimal MP4 writer for one H.264 video and one Opus audio track, for
//! exports that must open in QuickTime, PowerPoint and LMS uploaders.
//!
//! Sample data is streamed to a temporary file while the sample tables are
//! kept in memory; `finish` writes `ftyp` + `moov` + `mdat` so the index comes
//! first ("faststart") and the file plays while still downloading.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::mkv::{AudioSpec, OPUS_PRE_SKIP, OPUS_SAMPLE_RATE, Track, VideoSpec};

/// Video media timescale: divisible by every common frame rate.
const VIDEO_TIMESCALE: u32 = 90_000;
/// Every Opus packet we write is 20 ms.
const OPUS_FRAME: u32 = OPUS_SAMPLE_RATE / 50;
const MOVIE_TIMESCALE: u32 = 1000;
const MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

struct Sample {
    offset: u64,
    size: u32,
    t_ms: u64,
    sync: bool,
}

pub struct Mp4Writer {
    tmp_path: PathBuf,
    tmp: BufWriter<File>,
    written: u64,
    video: Option<(VideoSpec, Vec<Sample>)>,
    audio: Option<(AudioSpec, Vec<Sample>)>,
}

impl Mp4Writer {
    /// `tmp_path` holds sample data until `finish`; it is removed afterwards.
    pub fn new(tmp_path: &Path, video: Option<VideoSpec>, audio: Option<AudioSpec>) -> io::Result<Self> {
        Ok(Self {
            tmp_path: tmp_path.to_path_buf(),
            tmp: BufWriter::new(File::create(tmp_path)?),
            written: 0,
            video: video.map(|v| (v, Vec::new())),
            audio: audio.map(|a| (a, Vec::new())),
        })
    }

    /// Append a sample. Video data is 4-byte length-prefixed NAL units (as
    /// stored in Matroska); audio is one Opus packet.
    pub fn write(&mut self, track: Track, t_ms: u64, keyframe: bool, data: &[u8]) -> io::Result<()> {
        let samples = match track {
            Track::Video => self.video.as_mut().map(|(_, s)| s),
            Track::Audio => self.audio.as_mut().map(|(_, s)| s),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("no {track:?} track")))?;
        if let Some(last) = samples.last()
            && t_ms < last.t_ms
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "samples must be in time order"));
        }
        samples.push(Sample { offset: self.written, size: data.len() as u32, t_ms, sync: keyframe || track == Track::Audio });
        self.tmp.write_all(data)?;
        self.written += data.len() as u64;
        Ok(())
    }

    pub fn finish(mut self, out: &Path, duration_ms: u64) -> io::Result<()> {
        self.tmp.flush()?;
        drop(self.tmp);

        let mut ftyp = Vec::new();
        bx(&mut ftyp, b"ftyp", |b| {
            b.extend_from_slice(b"isom");
            u32_(b, 0x200);
            for brand in [b"isom", b"iso2", b"avc1", b"mp41"] {
                b.extend_from_slice(brand);
            }
        });
        // The moov's size doesn't depend on the offsets inside it, so build it
        // once to measure, then again with the real data offset.
        const MDAT_HEADER: u64 = 16;
        let probe = moov(&self.video, &self.audio, duration_ms, 0);
        let base = ftyp.len() as u64 + probe.len() as u64 + MDAT_HEADER;
        let moov = moov(&self.video, &self.audio, duration_ms, base);
        debug_assert_eq!(moov.len(), probe.len());

        let mut f = BufWriter::new(File::create(out)?);
        f.write_all(&ftyp)?;
        f.write_all(&moov)?;
        // 64-bit "largesize" mdat header so long recordings never overflow.
        u32_w(&mut f, 1)?;
        f.write_all(b"mdat")?;
        f.write_all(&(MDAT_HEADER + self.written).to_be_bytes())?;
        io::copy(&mut File::open(&self.tmp_path)?, &mut f)?;
        f.flush()?;
        fs::remove_file(&self.tmp_path)?;
        Ok(())
    }
}

fn moov(
    video: &Option<(VideoSpec, Vec<Sample>)>,
    audio: &Option<(AudioSpec, Vec<Sample>)>,
    duration_ms: u64,
    base: u64,
) -> Vec<u8> {
    let tracks = video.is_some() as u32 + audio.is_some() as u32;
    let mut out = Vec::new();
    bx(&mut out, b"moov", |b| {
        full(b, b"mvhd", 0, 0, |b| {
            u32_(b, 0);
            u32_(b, 0);
            u32_(b, MOVIE_TIMESCALE);
            u32_(b, duration_ms as u32);
            u32_(b, 0x0001_0000);
            u16_(b, 0x0100);
            b.extend_from_slice(&[0; 10]);
            MATRIX.iter().for_each(|m| u32_(b, *m));
            b.extend_from_slice(&[0; 24]);
            u32_(b, tracks + 1);
        });
        if let Some((spec, samples)) = video {
            trak_video(b, 1, spec, samples, duration_ms, base);
        }
        if let Some((spec, samples)) = audio {
            trak_audio(b, 1 + video.is_some() as u32, spec, samples, duration_ms, base);
        }
    });
    out
}

fn tkhd(b: &mut Vec<u8>, id: u32, duration_ms: u64, volume: u16, w: u32, h: u32) {
    full(b, b"tkhd", 0, 3, |b| {
        u32_(b, 0);
        u32_(b, 0);
        u32_(b, id);
        u32_(b, 0);
        u32_(b, duration_ms as u32);
        b.extend_from_slice(&[0; 8]);
        u16_(b, 0);
        u16_(b, 0);
        u16_(b, volume);
        u16_(b, 0);
        MATRIX.iter().for_each(|m| u32_(b, *m));
        u32_(b, w << 16);
        u32_(b, h << 16);
    });
}

fn mdhd_hdlr(b: &mut Vec<u8>, timescale: u32, duration: u64, handler: &[u8; 4], name: &str) {
    full(b, b"mdhd", 0, 0, |b| {
        u32_(b, 0);
        u32_(b, 0);
        u32_(b, timescale);
        u32_(b, duration as u32);
        u16_(b, 0x55C4); // "und"
        u16_(b, 0);
    });
    full(b, b"hdlr", 0, 0, |b| {
        u32_(b, 0);
        b.extend_from_slice(handler);
        b.extend_from_slice(&[0; 12]);
        b.extend_from_slice(name.as_bytes());
        b.push(0);
    });
}

fn dinf(b: &mut Vec<u8>) {
    bx(b, b"dinf", |b| {
        full(b, b"dref", 0, 0, |b| {
            u32_(b, 1);
            full(b, b"url ", 0, 1, |_| {});
        });
    });
}

/// stts, (stss), stsc, stsz, co64 with one sample per chunk.
fn tables(b: &mut Vec<u8>, samples: &[Sample], deltas: &[u32], with_stss: bool, base: u64) {
    full(b, b"stts", 0, 0, |b| {
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for &d in deltas {
            match runs.last_mut() {
                Some((n, last)) if *last == d => *n += 1,
                _ => runs.push((1, d)),
            }
        }
        u32_(b, runs.len() as u32);
        for (n, d) in runs {
            u32_(b, n);
            u32_(b, d);
        }
    });
    if with_stss {
        full(b, b"stss", 0, 0, |b| {
            let sync: Vec<u32> = samples.iter().enumerate().filter(|(_, s)| s.sync).map(|(i, _)| i as u32 + 1).collect();
            u32_(b, sync.len() as u32);
            sync.iter().for_each(|i| u32_(b, *i));
        });
    }
    full(b, b"stsc", 0, 0, |b| {
        u32_(b, 1);
        u32_(b, 1);
        u32_(b, 1);
        u32_(b, 1);
    });
    full(b, b"stsz", 0, 0, |b| {
        u32_(b, 0);
        u32_(b, samples.len() as u32);
        samples.iter().for_each(|s| u32_(b, s.size));
    });
    full(b, b"co64", 0, 0, |b| {
        u32_(b, samples.len() as u32);
        samples.iter().for_each(|s| b.extend_from_slice(&(base + s.offset).to_be_bytes()));
    });
}

fn trak_video(b: &mut Vec<u8>, id: u32, spec: &VideoSpec, samples: &[Sample], duration_ms: u64, base: u64) {
    let ticks = |ms: u64| ms * (VIDEO_TIMESCALE as u64 / 1000);
    // Variable frame rate: each sample lasts until the next; the last one
    // until the end of the movie.
    let deltas: Vec<u32> = samples
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let next = samples.get(i + 1).map_or(duration_ms.max(s.t_ms + 1), |n| n.t_ms);
            ticks(next - s.t_ms).max(1) as u32
        })
        .collect();
    bx(b, b"trak", |b| {
        tkhd(b, id, duration_ms, 0, spec.width, spec.height);
        bx(b, b"mdia", |b| {
            mdhd_hdlr(b, VIDEO_TIMESCALE, ticks(duration_ms), b"vide", "OpenSCRIB video");
            bx(b, b"minf", |b| {
                full(b, b"vmhd", 0, 1, |b| b.extend_from_slice(&[0; 8]));
                dinf(b);
                bx(b, b"stbl", |b| {
                    full(b, b"stsd", 0, 0, |b| {
                        u32_(b, 1);
                        bx(b, b"avc1", |b| {
                            b.extend_from_slice(&[0; 6]);
                            u16_(b, 1);
                            b.extend_from_slice(&[0; 16]);
                            u16_(b, spec.width as u16);
                            u16_(b, spec.height as u16);
                            u32_(b, 0x0048_0000);
                            u32_(b, 0x0048_0000);
                            u32_(b, 0);
                            u16_(b, 1);
                            b.extend_from_slice(&[0; 32]);
                            u16_(b, 0x0018);
                            u16_(b, 0xFFFF);
                            bx(b, b"avcC", |b| b.extend_from_slice(&spec.avcc));
                        });
                    });
                    tables(b, samples, &deltas, true, base);
                });
            });
        });
    });
}

fn trak_audio(b: &mut Vec<u8>, id: u32, spec: &AudioSpec, samples: &[Sample], duration_ms: u64, base: u64) {
    let deltas = vec![OPUS_FRAME; samples.len()];
    let media_duration = samples.len() as u64 * OPUS_FRAME as u64;
    bx(b, b"trak", |b| {
        tkhd(b, id, duration_ms, 0x0100, 0, 0);
        bx(b, b"mdia", |b| {
            mdhd_hdlr(b, OPUS_SAMPLE_RATE, media_duration, b"soun", "OpenSCRIB narration");
            bx(b, b"minf", |b| {
                full(b, b"smhd", 0, 0, |b| b.extend_from_slice(&[0; 4]));
                dinf(b);
                bx(b, b"stbl", |b| {
                    full(b, b"stsd", 0, 0, |b| {
                        u32_(b, 1);
                        bx(b, b"Opus", |b| {
                            b.extend_from_slice(&[0; 6]);
                            u16_(b, 1);
                            b.extend_from_slice(&[0; 8]);
                            u16_(b, spec.channels as u16);
                            u16_(b, 16);
                            u16_(b, 0);
                            u16_(b, 0);
                            u32_(b, OPUS_SAMPLE_RATE << 16);
                            bx(b, b"dOps", |b| {
                                b.push(0);
                                b.push(spec.channels);
                                u16_(b, OPUS_PRE_SKIP);
                                u32_(b, OPUS_SAMPLE_RATE);
                                u16_(b, 0);
                                b.push(0);
                            });
                        });
                    });
                    tables(b, samples, &deltas, false, base);
                });
            });
        });
    });
}

fn bx(out: &mut Vec<u8>, typ: &[u8; 4], body: impl FnOnce(&mut Vec<u8>)) {
    let start = out.len();
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(typ);
    body(out);
    let size = (out.len() - start) as u32;
    out[start..start + 4].copy_from_slice(&size.to_be_bytes());
}

fn full(out: &mut Vec<u8>, typ: &[u8; 4], version: u8, flags: u32, body: impl FnOnce(&mut Vec<u8>)) {
    bx(out, typ, |b| {
        u32_(b, (version as u32) << 24 | (flags & 0x00FF_FFFF));
        body(b);
    });
}

fn u16_(b: &mut Vec<u8>, v: u16) {
    b.extend_from_slice(&v.to_be_bytes());
}

fn u32_(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_be_bytes());
}

fn u32_w(w: &mut impl Write, v: u32) -> io::Result<()> {
    w.write_all(&v.to_be_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mkv::avcc;
    use mp4parse::{AudioCodecSpecific, SampleEntry, TrackType, VideoCodecSpecific, read_mp4};

    const SPS: &[u8] = &[0x67, 0x42, 0xC0, 0x1F, 0x8C, 0x8D, 0x40];
    const PPS: &[u8] = &[0x68, 0xCE, 0x3C, 0x80];

    #[test]
    fn mozilla_parser_reads_our_output() {
        let tmp = tempfile::tempdir().unwrap();
        let v = VideoSpec { width: 1920, height: 800, avcc: avcc(SPS, PPS) };
        let mut w = Mp4Writer::new(&tmp.path().join("data.tmp"), Some(v), Some(AudioSpec { channels: 1 })).unwrap();
        // Interleaved like an export: video at 0, 100, 1000 ms; audio every 20 ms.
        let mut vt = [0u64, 100, 1000].into_iter().peekable();
        for t in (0..2000).step_by(20) {
            while vt.peek().is_some_and(|v| *v <= t) {
                let v = vt.next().unwrap();
                w.write(Track::Video, v, v == 0, &[0, 0, 0, 2, 0x65, v as u8]).unwrap();
            }
            w.write(Track::Audio, t, true, &[0xFC, 1, 2]).unwrap();
        }
        let out = tmp.path().join("out.mp4");
        w.finish(&out, 2000).unwrap();
        assert!(!tmp.path().join("data.tmp").exists());

        let bytes = fs::read(&out).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
        // Faststart: the index precedes the media data.
        let moov = bytes.windows(4).position(|w| w == b"moov").unwrap();
        let mdat = bytes.windows(4).position(|w| w == b"mdat").unwrap();
        assert!(moov < mdat);

        let ctx = read_mp4(&mut std::io::Cursor::new(&bytes)).unwrap();
        assert_eq!(ctx.tracks.len(), 2);
        let vt = &ctx.tracks[0];
        assert_eq!(vt.track_type, TrackType::Video);
        let stsd = vt.stsd.as_ref().unwrap();
        let SampleEntry::Video(entry) = &stsd.descriptions[0] else { panic!("not video") };
        assert_eq!((entry.width, entry.height), (1920, 800));
        let VideoCodecSpecific::AVCConfig(cfg) = &entry.codec_specific else { panic!("no avcC") };
        assert_eq!(&cfg[..], &avcc(SPS, PPS)[..]);
        let deltas: Vec<_> = vt.stts.as_ref().unwrap().samples.iter().map(|s| (s.sample_count, s.sample_delta)).collect();
        assert_eq!(deltas, [(1, 9000), (1, 81000), (1, 90000)]);
        assert_eq!(&vt.stss.as_ref().unwrap().samples[..], &[1]);

        let at = &ctx.tracks[1];
        assert_eq!(at.track_type, TrackType::Audio);
        let SampleEntry::Audio(entry) = &at.stsd.as_ref().unwrap().descriptions[0] else { panic!("not audio") };
        assert!(matches!(entry.codec_specific, AudioCodecSpecific::OpusSpecificBox(_)));
        assert_eq!(at.stsz.as_ref().unwrap().sample_sizes.len(), 100);

        // Every chunk offset points at the bytes we wrote for that sample.
        let offsets = &vt.stco.as_ref().unwrap().offsets;
        assert_eq!(&bytes[offsets[2] as usize..offsets[2] as usize + 6], &[0, 0, 0, 2, 0x65, 232]);
        let aoff = at.stco.as_ref().unwrap().offsets[99] as usize;
        assert_eq!(&bytes[aoff..aoff + 3], &[0xFC, 1, 2]);
    }
}
