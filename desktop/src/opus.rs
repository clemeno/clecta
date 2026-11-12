//! Opus, for the WebM files video sites serve (PLAN Q71).
//!
//! symphonia 0.5 demuxes WebM and names its Opus tracks, but ships no decoder for them, and
//! rodio decodes with symphonia's registry of decoders. This module is the missing decoder —
//! `opus-pure`, pure Rust, behind symphonia's `Decoder` trait — and `install`, which hands
//! rodio a registry holding symphonia's own decoders plus this one.

use opus_pure::{MAX_PACKET_SAMPLES, OpusDecoder};
use symphonia::core::audio::{
	AsAudioBufferRef, AudioBuffer, AudioBufferRef, Channels, Signal, SignalSpec,
};
use symphonia::core::codecs::{
	CODEC_TYPE_OPUS, CodecDescriptor, CodecParameters, CodecRegistry, Decoder, DecoderOptions,
	FinalizeResult,
};
use symphonia::core::errors::{Error, Result, decode_error, unsupported_error};
use symphonia::core::formats::Packet;
use symphonia::core::support_codec;

/// The rate Opus decodes at here: its native one, so nothing is resampled on the way out.
const RATE: u32 = 48_000;

/// Give rodio symphonia's decoders plus Opus.
///
/// Called from `audio::decoder`, the one door every decode goes through, so neither the app
/// nor a test can open a file before it has run. The first call builds the registry; every
/// later one is a load and a branch.
pub fn install() {
	rodio::decoder::symphonia::CODECS.get_or_init(|| {
		let mut registry = CodecRegistry::new();
		symphonia::default::register_enabled_codecs(&mut registry);
		registry.register_all::<Opus>();
		registry
	});
}

/// symphonia's `Decoder`, decoding with `opus-pure`.
struct Opus {
	decoder: OpusDecoder,
	params: CodecParameters,
	/// One packet's sound, planar, as symphonia hands it on.
	buffer: AudioBuffer<f32>,
	/// One packet's sound, interleaved, as `opus-pure` writes it.
	interleaved: Vec<f32>,
	/// The header's output gain, as a factor: what the encoder asks every sample be scaled by.
	gain: f32,
}

impl Decoder for Opus {
	fn try_new(params: &CodecParameters, _options: &DecoderOptions) -> Result<Self> {
		// The identification header (RFC 7845 §5.1), which WebM carries as the track's
		// CodecPrivate and symphonia hands on as the extra data: byte 8 the version, 9 the
		// channel count, 10–11 the pre-skip, 16–17 the output gain, 18 the mapping family.
		let head = params.extra_data.as_deref().unwrap_or_default();
		if head.len() < 19 || !head.starts_with(b"OpusHead") {
			return decode_error("opus: no identification header");
		}
		// A new major version is a header this layout no longer describes (RFC 7845 §5.1).
		if head[8] & 0xf0 != 0 {
			return unsupported_error("opus: an unknown header version");
		}
		let channels = usize::from(head[9]);
		// Q7.8 decibels, applied as the RFC asks; video sites write zero, which is a factor of 1.
		let decibels = f32::from(i16::from_le_bytes([head[16], head[17]])) / 256.0;

		// `ponytail:` the pre-skip (bytes 10–11, 6.5 ms of encoder priming) is decoded and
		// played, not dropped. symphonia's MKV demuxer ignores the track's CodecDelay, so
		// dropping it from the start alone would put every seek 6.5 ms off the scan's timeline;
		// dropping it everywhere means shifting seek targets inside rodio. Played, the decode,
		// the container's length and every seek share one zero. And no 80 ms pre-roll after a
		// seek (RFC 7845 §4.6): the first few milliseconds after a scrub may be softer.

		// `ponytail:` channel mapping family 0 only — one stream, mono or stereo, which is
		// every music file a video site serves. Surround (family 1) needs `opus_pure::
		// OpusMSDecoder` and the header's channel map; until then such a file is "cannot decode".
		let layout = match (head[18], channels) {
			(0, 1) => Channels::FRONT_LEFT,
			(0, 2) => Channels::FRONT_LEFT | Channels::FRONT_RIGHT,
			_ => return unsupported_error("opus: only mono and stereo"),
		};
		let decoder = OpusDecoder::new(RATE as i32, channels)
			.map_err(|_| Error::DecodeError("opus: cannot start a decoder"))?;

		Ok(Self {
			decoder,
			params: params.clone(),
			buffer: AudioBuffer::new(MAX_PACKET_SAMPLES as u64, SignalSpec::new(RATE, layout)),
			interleaved: vec![0.0; MAX_PACKET_SAMPLES * channels],
			gain: 10f32.powf(decibels / 20.0),
		})
	}

	fn supported_codecs() -> &'static [CodecDescriptor] {
		&[support_codec!(CODEC_TYPE_OPUS, "opus", "Opus")]
	}

	fn reset(&mut self) {
		// Clearing state the decoder already holds has nothing to fail on.
		let _ = self.decoder.reset_state();
	}

	fn codec_params(&self) -> &CodecParameters {
		&self.params
	}

	fn decode(&mut self, packet: &Packet) -> Result<AudioBufferRef<'_>> {
		// An empty packet means a lost one to `opus-pure`, which would conceal a whole
		// `MAX_PACKET_SAMPLES` (120 ms) of it; rodio skips a packet that errors instead.
		if packet.data.is_empty() {
			return decode_error("opus: an empty packet");
		}
		let frames = self
			.decoder
			.decode(&packet.data, MAX_PACKET_SAMPLES, &mut self.interleaved)
			.map_err(|_| Error::DecodeError("opus: cannot decode a packet"))?;

		let channels = self.buffer.spec().channels.count();
		self.buffer.clear();
		self.buffer.render_reserved(Some(frames));
		for channel in 0..channels {
			for (frame, sample) in self.buffer.chan_mut(channel).iter_mut().enumerate() {
				*sample = self.interleaved[frame * channels + channel] * self.gain;
			}
		}
		// The container's trims, as symphonia's own decoders apply them: Ogg states where the
		// sound ends inside the last packet (PLAN Q72). Matroska states none.
		self.buffer
			.trim(packet.trim_start as usize, packet.trim_end as usize);

		Ok(self.buffer.as_audio_buffer_ref())
	}

	fn finalize(&mut self) -> FinalizeResult {
		FinalizeResult::default()
	}

	fn last_decoded(&self) -> AudioBufferRef<'_> {
		self.buffer.as_audio_buffer_ref()
	}
}
