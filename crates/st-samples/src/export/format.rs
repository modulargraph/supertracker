//! Export file policy: which encodings and delivery sample rates exist, and how a request
//! selects them. Validate with [`ExportFormat::resolve`] before rendering or creating files.
//!
//! Exports are WAV files. The render itself is always 32-bit float at the engine's rate;
//! conversion to the delivery rate and encoding happens afterwards ([`super::DeliveryJob`]).

use serde::{Deserialize, Serialize};

use crate::error::{Result, SampleError};
use crate::wav::{Dither, WavEncoding};

/// Sample rate the engine renders at unless the caller says otherwise.
pub const RENDER_SAMPLE_RATE: u32 = 48_000;
/// Delivery rate when a request names none.
pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;
/// Delivery rates an export can ask for.
pub const SUPPORTED_SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 96_000];

/// How an exported WAV file stores its samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportEncoding {
    /// 16-bit integer PCM.
    Pcm16,
    /// 24-bit integer PCM (the default).
    Pcm24,
    /// 32-bit float.
    Float32,
}

impl ExportEncoding {
    /// Every encoding, in the order the interface lists them.
    pub const ALL: [ExportEncoding; 3] = [ExportEncoding::Pcm16, ExportEncoding::Pcm24, ExportEncoding::Float32];
    /// The encoding used when a request names none.
    pub const DEFAULT: ExportEncoding = ExportEncoding::Pcm24;

    /// The wire name: "pcm16", "pcm24" or "float32".
    pub fn name(self) -> &'static str {
        match self {
            ExportEncoding::Pcm16 => "pcm16",
            ExportEncoding::Pcm24 => "pcm24",
            ExportEncoding::Float32 => "float32",
        }
    }

    /// The encoding with exactly this wire name.
    pub fn from_name(name: &str) -> Option<ExportEncoding> {
        Self::ALL.into_iter().find(|encoding| encoding.name() == name)
    }

    /// Bits per sample.
    pub fn bit_depth(self) -> u32 {
        self.wav_encoding().bit_depth()
    }

    /// True for 32-bit float.
    pub fn is_float(self) -> bool {
        self == ExportEncoding::Float32
    }

    /// The matching WAV sample encoding.
    pub fn wav_encoding(self) -> WavEncoding {
        match self {
            ExportEncoding::Pcm16 => WavEncoding::Pcm16,
            ExportEncoding::Pcm24 => WavEncoding::Pcm24,
            ExportEncoding::Float32 => WavEncoding::Float32,
        }
    }
}

/// What is being exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportMode {
    /// The whole song as one file.
    Song,
    /// One pattern as one file.
    Pattern,
    /// One file per track and bus, in a folder.
    Stems,
    /// A track bounce: an internal editable print, always 32-bit float at the render rate
    /// whatever the request says.
    Track,
}

impl ExportMode {
    /// "song", "pattern", "stems" or "track"; anything else is an error.
    pub fn from_name(name: &str) -> Result<ExportMode> {
        match name {
            "song" => Ok(ExportMode::Song),
            "pattern" => Ok(ExportMode::Pattern),
            "stems" => Ok(ExportMode::Stems),
            "track" => Ok(ExportMode::Track),
            _ => Err(SampleError::invalid("Choose song, pattern, stems, or track export.")),
        }
    }
}

/// The format fields of an export request. Absent fields take the defaults; present fields
/// must be valid even when the mode overrides them.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ExportFormatRequest {
    /// Only "wav" exists.
    pub container: Option<String>,
    /// "pcm16", "pcm24" or "float32".
    pub encoding: Option<String>,
    /// 44100, 48000 or 96000.
    pub sample_rate: Option<f64>,
    /// "none" (default) or "tpdf"; applies to the integer encodings.
    pub dither: Option<String>,
}

/// A validated export format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportFormat {
    /// Sample encoding of the delivered file.
    pub encoding: ExportEncoding,
    /// Sample rate of the delivered file.
    pub sample_rate: u32,
    /// Dither applied when the encoding is an integer one.
    pub dither: Dither,
    /// Sample rate of the render the file is made from.
    pub render_sample_rate: u32,
}

/// The description of an export's format that travels with its result.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportMetadata {
    /// Always "wav".
    pub container: &'static str,
    /// Wire name of the encoding.
    pub encoding: &'static str,
    /// Delivered sample rate.
    pub sample_rate: u32,
    /// Bits per sample.
    pub bit_depth: u32,
    /// True for float samples.
    pub is_float: bool,
    /// "none" or "tpdf".
    pub dither: &'static str,
    /// Sample rate of the render.
    pub render_sample_rate: u32,
}

/// What the export dialog can offer.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCapabilities {
    /// Always "wav".
    pub container: &'static str,
    /// The render rate.
    pub sample_rate: u32,
    /// Delivery rates.
    pub sample_rates: Vec<u32>,
    /// Delivery rate when none is requested.
    pub default_sample_rate: u32,
    /// Wire names of the encodings.
    pub encodings: Vec<&'static str>,
    /// Encoding when none is requested.
    pub default_encoding: &'static str,
    /// Dither choices for the integer encodings.
    pub dithers: Vec<&'static str>,
    /// Dither when none is requested.
    pub default_dither: &'static str,
}

/// The export formats this build writes, for the interface.
pub fn capabilities() -> ExportCapabilities {
    ExportCapabilities {
        container: "wav",
        sample_rate: RENDER_SAMPLE_RATE,
        sample_rates: SUPPORTED_SAMPLE_RATES.to_vec(),
        default_sample_rate: DEFAULT_SAMPLE_RATE,
        encodings: ExportEncoding::ALL.iter().map(|encoding| encoding.name()).collect(),
        default_encoding: ExportEncoding::DEFAULT.name(),
        dithers: vec!["none", "tpdf"],
        default_dither: "none",
    }
}

fn dither_name(dither: Dither) -> &'static str {
    match dither {
        Dither::None => "none",
        Dither::Tpdf => "tpdf",
    }
}

/// The delivery rate a number names: exactly 44100, 48000 or 96000.
pub fn delivery_sample_rate(value: f64) -> Result<u32> {
    SUPPORTED_SAMPLE_RATES
        .into_iter()
        .find(|&rate| value == rate as f64)
        .ok_or_else(|| SampleError::invalid("Choose a 44.1, 48, or 96 kHz WAV delivery rate."))
}

impl ExportFormat {
    /// Validates a request for the export `mode` ("song", "pattern", "stems" or "track") of a
    /// render at `render_sample_rate`.
    ///
    /// Every field that is present must be valid. A track bounce then ignores them: it is always
    /// 32-bit float at the render rate, so that a bounce never loses precision.
    pub fn resolve(request: &ExportFormatRequest, mode: &str, render_sample_rate: u32) -> Result<ExportFormat> {
        let mode = ExportMode::from_name(mode)?;
        if request.container.as_deref().is_some_and(|container| container != "wav") {
            return Err(SampleError::invalid("Audio export supports WAV files only."));
        }
        let sample_rate = match request.sample_rate {
            Some(value) => delivery_sample_rate(value)?,
            None => DEFAULT_SAMPLE_RATE,
        };
        let encoding = match request.encoding.as_deref() {
            Some(name) => ExportEncoding::from_name(name).ok_or_else(|| SampleError::invalid("Choose PCM 16-bit, PCM 24-bit, or 32-bit float WAV encoding."))?,
            None => ExportEncoding::DEFAULT,
        };
        let dither = match request.dither.as_deref() {
            None | Some("none") => Dither::None,
            Some("tpdf") => Dither::Tpdf,
            Some(_) => return Err(SampleError::invalid("Choose no dither or TPDF dither.")),
        };
        if mode == ExportMode::Track {
            return Ok(ExportFormat { encoding: ExportEncoding::Float32, sample_rate: render_sample_rate, dither: Dither::None, render_sample_rate });
        }
        let dither = if encoding.is_float() { Dither::None } else { dither };
        Ok(ExportFormat { encoding, sample_rate, dither, render_sample_rate })
    }

    /// The description that accompanies the exported file.
    pub fn metadata(&self) -> ExportMetadata {
        ExportMetadata {
            container: "wav",
            encoding: self.encoding.name(),
            sample_rate: self.sample_rate,
            bit_depth: self.encoding.bit_depth(),
            is_float: self.encoding.is_float(),
            dither: dither_name(self.dither),
            render_sample_rate: self.render_sample_rate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(json: &str) -> ExportFormatRequest {
        serde_json::from_str(json).unwrap()
    }

    fn resolve(json: &str, mode: &str) -> Result<ExportFormat> {
        ExportFormat::resolve(&request(json), mode, 48_000)
    }

    #[test]
    fn defaults_and_every_rate_encoding_mode_combination() {
        for mode in ["song", "pattern", "stems"] {
            let implicit = resolve("{}", mode).unwrap();
            assert_eq!((implicit.encoding, implicit.sample_rate, implicit.dither), (ExportEncoding::Pcm24, 48_000, Dither::None));
        }
        let mut cases = 0;
        for mode in ["song", "pattern", "stems", "track"] {
            for rate in SUPPORTED_SAMPLE_RATES {
                for encoding in ExportEncoding::ALL {
                    // Integer and floating-point spellings of the same number are the same rate.
                    for number in [format!("{rate}"), format!("{rate}.0"), format!("{}e3", rate as f64 / 1000.0)] {
                        let json = format!(r#"{{"sampleRate":{number},"encoding":"{}","container":"wav"}}"#, encoding.name());
                        let format = resolve(&json, mode).unwrap();
                        if mode == "track" {
                            assert_eq!((format.encoding, format.sample_rate), (ExportEncoding::Float32, 48_000), "a bounce stays float at the render rate");
                            assert!(format.encoding.bit_depth() == 32 && format.encoding.is_float());
                        } else {
                            assert_eq!((format.encoding, format.sample_rate), (encoding, rate));
                        }
                        let metadata = format.metadata();
                        assert_eq!((metadata.container, metadata.encoding, metadata.render_sample_rate), ("wav", format.encoding.name(), 48_000));
                        assert_eq!((metadata.bit_depth, metadata.is_float), (format.encoding.bit_depth(), format.encoding.is_float()));
                        cases += 1;
                    }
                }
            }
        }
        assert_eq!(cases, 108);
        assert_eq!(resolve("{}", "track").unwrap().encoding, ExportEncoding::Float32);
        assert_eq!(ExportFormat::resolve(&ExportFormatRequest::default(), "track", 44_100).unwrap().sample_rate, 44_100);
    }

    #[test]
    fn malformed_values_are_rejected_in_every_mode() {
        for mode in ["song", "pattern", "stems", "track"] {
            for encoding in ["", "pcm32", "PCM24", " pcm24", "pcm24 ", "wav"] {
                let error = resolve(&format!(r#"{{"encoding":"{encoding}"}}"#), mode).unwrap_err();
                assert!(!error.to_string().is_empty());
            }
            for rate in ["22050", "88200", "192000", "0", "-48000", "44100.25", "48000.5", "96000.5", "1e400", "48000.0000001"] {
                // 1e400 does not parse as a finite JSON number at all; the rest must fail validation.
                match serde_json::from_str::<ExportFormatRequest>(&format!(r#"{{"sampleRate":{rate}}}"#)) {
                    Ok(request) => assert!(ExportFormat::resolve(&request, mode, 48_000).is_err(), "rate {rate} accepted in {mode}"),
                    Err(_) => assert_eq!(rate, "1e400"),
                }
            }
            for rate in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                let request = ExportFormatRequest { sample_rate: Some(rate), ..Default::default() };
                assert!(ExportFormat::resolve(&request, mode, 48_000).is_err());
                assert!(delivery_sample_rate(rate).is_err());
            }
            assert!(resolve(r#"{"dither":"loud"}"#, mode).is_err());
        }
        for container in ["WAV", "aiff", "", "wav ", "flac"] {
            assert_eq!(resolve(&format!(r#"{{"container":"{container}"}}"#), "song").unwrap_err().to_string(), "Audio export supports WAV files only.");
        }
        for mode in ["", "bounce", "Song", "stems "] {
            assert!(resolve("{}", mode).is_err(), "mode {mode:?} accepted");
        }
    }

    #[test]
    fn json_types_stay_distinct() {
        // The bridge deserializes into this struct: wrong JSON types never reach validation.
        for json in [
            r#"{"sampleRate":true}"#,
            r#"{"sampleRate":"48000"}"#,
            r#"{"sampleRate":[48000]}"#,
            r#"{"sampleRate":{"rate":48000}}"#,
            r#"{"encoding":16}"#,
            r#"{"encoding":true}"#,
            r#"{"encoding":["pcm24"]}"#,
            r#"{"container":1}"#,
            r#"{"container":true}"#,
        ] {
            assert!(serde_json::from_str::<ExportFormatRequest>(json).is_err(), "{json} deserialized");
        }
        let parsed = resolve(r#"{"encoding":"pcm16","sampleRate":48000,"path":"/x.wav","mode":"song"}"#, "song").unwrap();
        assert_eq!(parsed.encoding, ExportEncoding::Pcm16);
        for rate in SUPPORTED_SAMPLE_RATES {
            assert_eq!(resolve(&format!(r#"{{"sampleRate":{rate}}}"#), "stems").unwrap().sample_rate, rate);
        }
    }

    #[test]
    fn dither_applies_to_integer_deliveries_only() {
        assert_eq!(resolve(r#"{"encoding":"pcm16","dither":"tpdf"}"#, "song").unwrap().dither, Dither::Tpdf);
        assert_eq!(resolve(r#"{"encoding":"pcm16","dither":"none"}"#, "song").unwrap().dither, Dither::None);
        assert_eq!(resolve(r#"{"encoding":"float32","dither":"tpdf"}"#, "song").unwrap().dither, Dither::None);
        assert_eq!(resolve(r#"{"encoding":"pcm16","dither":"tpdf"}"#, "track").unwrap().dither, Dither::None);
        assert_eq!(resolve(r#"{"encoding":"pcm24","dither":"tpdf"}"#, "stems").unwrap().metadata().dither, "tpdf");
    }

    #[test]
    fn advertised_capabilities() {
        let json = serde_json::to_value(capabilities()).unwrap();
        assert_eq!(json["container"], "wav");
        assert_eq!(json["encodings"], serde_json::json!(["pcm16", "pcm24", "float32"]));
        assert_eq!(json["defaultEncoding"], "pcm24");
        assert_eq!(json["sampleRate"], 48_000);
        assert_eq!(json["sampleRates"], serde_json::json!([44_100, 48_000, 96_000]));
        assert_eq!(json["defaultSampleRate"], 48_000);
        assert_eq!(json["dithers"], serde_json::json!(["none", "tpdf"]));
        let metadata = serde_json::to_value(resolve(r#"{"encoding":"pcm16","sampleRate":44100}"#, "song").unwrap().metadata()).unwrap();
        assert_eq!(metadata, serde_json::json!({"container":"wav","encoding":"pcm16","sampleRate":44100,"bitDepth":16,"isFloat":false,"dither":"none","renderSampleRate":48000}));
        assert_eq!(serde_json::to_value(ExportEncoding::Float32).unwrap(), "float32");
    }
}
