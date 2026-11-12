use core::time::Duration;
use std::sync::Arc;
use symphonia::{
    core::{
        audio::{AudioBufferRef, SampleBuffer, SignalSpec},
        codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL},
        errors::Error,
        formats::{FormatOptions, FormatReader, Packet, SeekMode, SeekTo, SeekedTo}, // clecta patch: Packet
        io::MediaSourceStream,
        meta::MetadataOptions,
        probe::Hint,
        units,
    },
    default::get_probe,
};

use super::{DecoderError, Settings};
use crate::{
    common::{assert_error_traits, ChannelCount, Sample, SampleRate},
    source, Source,
};

// clecta patch: decoders beyond symphonia's own, registered by the application before the
// first file is opened. Unset, symphonia's defaults are used, as upstream does.
pub static CODECS: std::sync::OnceLock<symphonia::core::codecs::CodecRegistry> =
    std::sync::OnceLock::new();

pub(crate) struct SymphoniaDecoder {
    decoder: Box<dyn Decoder>,
    current_span_offset: usize,
    format: Box<dyn FormatReader>,
    // clecta patch: the track this decoder decodes, for the length, the seeks and the packets.
    track_id: u32,
    total_duration: Option<Duration>,
    buffer: SampleBuffer<Sample>,
    spec: SignalSpec,
    seek_mode: SeekMode,
    // clecta patch: where the decode is on the track's clock, for `span` and `refine_position`.
    time_base: Option<units::TimeBase>,
    /// The track's first timestamp: where its timeline, its length and its seeks start.
    origin: u64,
    /// The frame the next buffer starts at; `None` at the start and after a seek, until a
    /// packet's timestamp says.
    frame: Option<u64>,
    /// The frame the container says the sound ends at.
    end: Option<u64>,
    /// The samples of `buffer` that are played: all of them, except in the packet `end` cuts.
    span_len: usize,
    /// The timestamp of the packet `buffer` was decoded from.
    span_ts: u64,
}

impl SymphoniaDecoder {
    pub(crate) fn new(mss: MediaSourceStream, settings: &Settings) -> Result<Self, DecoderError> {
        match SymphoniaDecoder::init(mss, settings) {
            Err(e) => match e {
                Error::IoError(e) => Err(DecoderError::IoError(e.to_string())),
                Error::DecodeError(e) => Err(DecoderError::DecodeError(e)),
                Error::SeekError(_) => {
                    unreachable!("Seek errors should not occur during initialization")
                }
                Error::Unsupported(_) => Err(DecoderError::UnrecognizedFormat),
                Error::LimitError(e) => Err(DecoderError::LimitError(e)),
                Error::ResetRequired => Err(DecoderError::ResetRequired),
            },
            Ok(Some(decoder)) => Ok(decoder),
            Ok(None) => Err(DecoderError::NoStreams),
        }
    }

    #[inline]
    pub(crate) fn into_inner(self) -> MediaSourceStream {
        self.format.into_inner()
    }

    fn init(
        mss: MediaSourceStream,
        settings: &Settings,
    ) -> symphonia::core::errors::Result<Option<SymphoniaDecoder>> {
        let mut hint = Hint::new();
        if let Some(ext) = settings.hint.as_ref() {
            hint.with_extension(ext);
        }
        if let Some(typ) = settings.mime_type.as_ref() {
            hint.mime_type(typ);
        }
        let format_opts: FormatOptions = FormatOptions {
            enable_gapless: settings.gapless,
            ..Default::default()
        };
        let metadata_opts: MetadataOptions = Default::default();
        let seek_mode = if settings.coarse_seek {
            SeekMode::Coarse
        } else {
            SeekMode::Accurate
        };
        let mut probed = get_probe().format(&hint, mss, &format_opts, &metadata_opts)?;

        // clecta patch: an empty container is still `None`, but its default track is no longer
        // the one decoded; `track`, below, is.
        if probed.format.default_track().is_none() {
            return Ok(None);
        }

        // Select the first supported track
        let track_id = probed
            .format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or(symphonia::core::errors::Error::Unsupported(
                "No track with supported codec",
            ))?
            .id;

        let track = match probed
            .format
            .tracks()
            .iter()
            .find(|track| track.id == track_id)
        {
            Some(track) => track,
            None => return Ok(None),
        };

        // clecta patch: the application's registry when it has given one, as `CODECS` says.
        let mut decoder = CODECS
            .get()
            .unwrap_or_else(symphonia::default::get_codecs)
            .make(&track.codec_params, &DecoderOptions::default())?;
        // clecta patch: the decoded track's clock and frame count, not the default track's (in an
        // MP4 the default track is the video, counted in its own time base); kept for the
        // length and for `span`, before `track`'s borrow of the format ends.
        let params = &track.codec_params;
        let (time_base, start_ts, n_frames) = (params.time_base, params.start_ts, params.n_frames);

        // clecta patch: and the packet's timestamp and undone trims with it, for `span`.
        let (decoded, ts, trim) = loop {
            let current_span = match probed.format.next_packet() {
                Ok(packet) => packet,
                Err(Error::IoError(_)) => break (decoder.last_decoded(), 0, (0, 0)), // clecta patch
                Err(e) => return Err(e),
            };

            // If the packet does not belong to the selected track, skip over it
            if current_span.track_id() != track_id {
                continue;
            }

            match decoder.decode(&current_span) {
                Ok(decoded) => {
                    // clecta patch
                    let trim = untrimmed(&current_span, decoded.frames());
                    break (decoded, current_span.ts(), trim);
                }
                Err(e) => match e {
                    Error::DecodeError(_) => {
                        // Decode errors are intentionally ignored with no retry limit.
                        // This behavior ensures that the decoder skips over problematic packets
                        // and continues processing the rest of the stream.
                        continue;
                    }
                    _ => return Err(e),
                },
            }
        };
        let spec = decoded.spec().to_owned();
        // clecta patch: the track's timeline starts at its first packet. Most start at zero, but
        // one cut from a longer stream can keep its timestamps, and then a Matroska length,
        // counted from zero, ran long by the gap, and an Ogg one, counted from the stream's
        // `start_ts`, did not (PLAN Q72). The length is from there to where the container ends.
        let origin = ts.max(start_ts);
        let total_duration: Option<Duration> = time_base
            .zip(n_frames)
            .map(|(base, frames)| base.calc_time((start_ts + frames).saturating_sub(origin)).into())
            .filter(|d: &Duration| !d.is_zero());
        // clecta patch: the end in frames, and the first buffer placed — and cut, in a file
        // shorter than one packet — exactly as `fill` places every later one.
        let frames = decoded.frames();
        let end = total_duration.map(|d| (d.as_secs_f64() * f64::from(spec.rate)).round() as u64);
        let buffer = SymphoniaDecoder::get_buffer(decoded, &spec);
        let mut this = SymphoniaDecoder { // clecta patch: placed by `span`, below
            decoder,
            current_span_offset: 0,
            format: probed.format,
            track_id, // clecta patch
            total_duration,
            buffer,
            spec,
            seek_mode,
            time_base,
            origin,
            frame: None,
            end,
            span_len: 0,
            span_ts: 0,
        };
        let kept = this.trim(frames, trim);
        this.span(ts, kept);
        Ok(Some(this))
    }

    // clecta patch: place a freshly decoded buffer of `frames` frames, from the packet at `ts`,
    // on the track, and cut it if it runs past the end the container states. That end is where
    // the sound stops inside the last packet: Opus and AAC code whole frames and pad the last,
    // and an MP4 or a Matroska file says where the padding starts only by its length, which no
    // symphonia 0.5 decoder of theirs applies (PLAN Q72). A packet wholly past the end is kept,
    // as the header is then what is wrong.
    fn span(&mut self, ts: u64, frames: u64) {
        let start = self.frame.unwrap_or_else(|| self.frame_at(ts));
        let kept = match self.end {
            Some(end) if start < end && end < start + frames => end - start,
            _ => frames,
        };
        self.span_len = kept as usize * self.spec.channels.count();
        self.span_ts = ts;
        self.frame = Some(start + frames);
    }

    // clecta patch: a timestamp, on the track's clock, as a frame at the decoded rate, counted
    // from the track's first packet as its length is.
    fn frame_at(&self, ts: u64) -> u64 {
        let rate = u64::from(self.spec.rate);
        self.time_base.map_or(0, |base| {
            let time = base.calc_time(ts.saturating_sub(self.origin));
            time.seconds * rate + (time.frac * rate as f64).round() as u64
        })
    }

    #[inline]
    fn get_buffer(decoded: AudioBufferRef, spec: &SignalSpec) -> SampleBuffer<Sample> {
        let duration = units::Duration::from(decoded.capacity() as u64);
        let mut buffer = SampleBuffer::<Sample>::new(duration, *spec);
        buffer.copy_interleaved_ref(decoded);
        buffer
    }
}

impl Source for SymphoniaDecoder {
    #[inline]
    fn current_span_len(&self) -> Option<usize> {
        Some(self.span_len) // clecta patch: what `span` kept
    }

    #[inline]
    fn channels(&self) -> ChannelCount {
        ChannelCount::new(
            self.spec
                .channels
                .count()
                .try_into()
                .expect("rodio only support up to u16::MAX channels (65_535)"),
        )
        .expect("audio should always have at least one channel")
    }

    #[inline]
    fn sample_rate(&self) -> SampleRate {
        SampleRate::new(self.spec.rate).expect("audio should always have a non zero SampleRate")
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), source::SeekError> {
        if matches!(self.seek_mode, SeekMode::Accurate)
            && self.decoder.codec_params().time_base.is_none()
        {
            return Err(source::SeekError::SymphoniaDecoder(
                SeekError::AccurateSeekNotSupported,
            ));
        }

        // Seeking should be "saturating", meaning: target positions beyond the end of the stream
        // are clamped to the end.
        let mut target = pos;
        if let Some(total_duration) = self.total_duration {
            if target > total_duration {
                target = total_duration;
            }
        }

        // Remember the current channel, so we can restore it after seeking.
        // clecta patch: `min`, as a seek that found nothing to decode leaves the offset at
        // `usize::MAX`, which is the second channel of two: left and right swapped.
        let active_channel =
            self.current_span_offset.min(self.span_len) % self.channels().get() as usize;

        // clecta patch: seek on the decoded track. `None` means the default track, whose
        // timestamps `refine_position` would then read in the wrong time base. And seek from
        // the track's first packet: `SeekTo::Time` counts from zero, so a track whose
        // timestamps start late refused every seek before its start and landed later ones
        // early by the gap (PLAN Q72).
        let to = match self.time_base {
            Some(base) => SeekTo::TimeStamp {
                ts: self.origin + base.calc_timestamp(target.into()),
                track_id: self.track_id,
            },
            None => SeekTo::Time {
                time: target.into(),
                track_id: Some(self.track_id), // clecta patch
            },
        };
        // clecta patch: `to`, above.
        let seek_res = match self.format.seek(self.seek_mode, to) {
            Err(Error::SeekError(symphonia::core::errors::SeekErrorKind::ForwardOnly)) => {
                return Err(source::SeekError::SymphoniaDecoder(
                    SeekError::RandomAccessNotSupported,
                ));
            }
            other => other.map_err(Arc::new).map_err(SeekError::Demuxer),
        }?;

        // Seeking is a demuxer operation without the decoder knowing about it,
        // so we need to reset the decoder to make sure it's in sync and prevent
        // audio glitches.
        self.decoder.reset();
        self.frame = None; // clecta patch: the next packet's timestamp says where the decode is

        // Force the iterator to decode the next packet.
        self.current_span_offset = usize::MAX;

        // Symphonia does not seek to the exact position, it seeks to the closest keyframe.
        // If accurate seeking is required, fast-forward to the exact position.
        if matches!(self.seek_mode, SeekMode::Accurate) {
            self.refine_position(seek_res)?;
        }

        // After seeking, we are at the beginning of an inter-sample frame, i.e. the first
        // channel. We need to advance the iterator to the right channel.
        for _ in 0..active_channel {
            self.next();
        }

        Ok(())
    }
}

/// Error returned when the try_seek implementation of the symphonia decoder fails.
#[derive(Debug, thiserror::Error, Clone)]
pub enum SeekError {
    /// Accurate seeking is not supported
    ///
    /// This error occurs when the decoder cannot extract time base information from the source.
    /// You may catch this error to try a coarse seek instead.
    #[error("Accurate seeking is not supported on this file/byte stream that lacks time base information")]
    AccurateSeekNotSupported,
    /// The decoder does not support random access seeking
    ///
    /// This error occurs when the source is not seekable or does not have a known byte length.
    #[error("The decoder needs to know the length of the file/byte stream to be able to seek backwards. You can set that by using the `DecoderBuilder` or creating a decoder using `Decoder::try_from(some_file)`.")]
    RandomAccessNotSupported,
    /// Demuxer failed to seek
    #[error("Demuxer failed to seek")]
    Demuxer(#[source] Arc<symphonia::core::errors::Error>),
}
assert_error_traits!(SeekError);

impl SymphoniaDecoder {
    /// Note span offset must be set after
    // clecta patch: count the skip from the first packet that gives sound, not from where the
    // demuxer landed, and make sure that packet starts at or before the target (PLAN Q72). Two
    // ways it did not: right after a reset a Vorbis packet gives no sound — it only primes the
    // overlap — so the sound began one packet past the landing; and a Matroska file without
    // block durations lands on the first block *after* the target. Either way the seek was up
    // to a packet late. So while the sound starts past the target, seek back from it and decode
    // again — once for Vorbis, once or twice for Matroska.
    fn refine_position(&mut self, seek_res: SeekedTo) -> Result<(), source::SeekError> {
        let mut ask = seek_res.actual_ts.min(seek_res.required_ts); // clecta patch: as above
        let mut step = 0;
        loop {
            if self.fill().is_none() {
                return Ok(());
            }
            if self.span_ts <= seek_res.required_ts || ask == 0 {
                break;
            }
            // Back first by as far as the sound started late, then by twice as far each try.
            step = match step {
                0 => self.span_ts - seek_res.required_ts,
                _ => step * 2,
            };
            ask = ask.saturating_sub(step);
            let to = SeekTo::TimeStamp {
                ts: ask,
                track_id: self.track_id,
            };
            match self.format.seek(SeekMode::Accurate, to) {
                Ok(to) if to.actual_ts <= self.span_ts => {}
                // A demuxer that cannot go back keeps the late landing, as upstream did. One that
                // only seeks forward (Matroska without cues) answers with the packet after the
                // one in hand, and asking again would land a packet later each time.
                _ => break,
            }
            self.decoder.reset();
            self.frame = None;
        }

        // Skip ahead to the precise position.
        // clecta patch: from the packet that gives sound, as above.
        let frames = self
            .frame_at(seek_res.required_ts)
            .saturating_sub(self.frame_at(self.span_ts));
        for _ in 0..frames * u64::from(self.channels().get()) {
            self.next();
        }

        Ok(())
    }

    // clecta patch: `next`'s decode, on its own so `refine_position` can decode a packet
    // without taking a sample from it. `None` at the end of the stream.
    fn fill(&mut self) -> Option<()> {
        let (decoded, ts, trim) = loop {
            let packet = self.format.next_packet().ok()?;
            // clecta patch: skip other tracks' packets, as `init` does.
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(Error::DecodeError(_)) => {
                    // Skip over packets that cannot be decoded. This ensures the iterator
                    // continues processing subsequent packets instead of terminating due to
                    // non-critical decode errors.
                    continue;
                }
                Err(_) => return None,
            };

            // Loop until we get a packet with audio frames. This is necessary because some
            // formats can have packets with only metadata, particularly when rewinding, in
            // which case the iterator would otherwise end with `None`.
            // Note: checking `decoded.frames()` is more reliable than `packet.dur()`, which
            // can resturn non-zero durations for packets without audio frames.
            // clecta patch: and frames left once the packet's trims are applied.
            let trim = untrimmed(&packet, decoded.frames());
            if decoded.frames() > trim.0 + trim.1 {
                break (decoded, packet.ts(), trim);
            }
        };

        decoded.spec().clone_into(&mut self.spec);
        let frames = decoded.frames();
        self.buffer = SymphoniaDecoder::get_buffer(decoded, &self.spec);
        self.current_span_offset = 0;
        let kept = self.trim(frames, trim);
        self.span(ts, kept);
        Some(())
    }

    // clecta patch: drop the frames a packet's trims cover, which its decoder left in (see
    // `untrimmed`), and say how many are kept.
    fn trim(&mut self, frames: usize, (start, end): (usize, usize)) -> u64 {
        if start > 0 {
            let channels = self.spec.channels.count();
            self.buffer
                .samples_mut()
                .copy_within(start * channels..(frames - end) * channels, 0);
        }
        (frames - start - end) as u64
    }
}

// clecta patch: the trims a decoder left undone, at the start and the end. symphonia's MP3 and
// Vorbis decoders, and `opus.rs`, apply a packet's trims themselves; its AAC and PCM ones return
// the whole block, which is how to tell. The MP4 reader's trims come from the edit list
// (PLAN Q72), and without these an AAC file's priming was played.
fn untrimmed(packet: &Packet, frames: usize) -> (usize, usize) {
    let (start, end) = (packet.trim_start() as usize, packet.trim_end() as usize);
    if start + end > 0 && frames as u64 == packet.block_dur() {
        (start, end)
    } else {
        (0, 0)
    }
}

impl Iterator for SymphoniaDecoder {
    type Item = Sample;

    fn next(&mut self) -> Option<Self::Item> {
        // clecta patch: `span_len`, not the buffer's length, as `span` may have cut it; and the
        // decode moved into `fill`.
        if self.current_span_offset >= self.span_len {
            self.fill()?;
        }

        let sample = *self.buffer.samples().get(self.current_span_offset)?;
        self.current_span_offset += 1;

        Some(sample)
    }
}
