use std::path::Path;

use lofty::prelude::*;
use lofty::read_from_path;

/// Audio metadata and properties extracted in a single read via `lofty`.
///
/// Covers every format `lofty` understands as audio — MP3, FLAC, Ogg Vorbis,
/// Opus, M4A/AAC/ALAC, WAV, AIFF, APE, WavPack, Musepack, and Speex — so the
/// audio fields are no longer limited to MP3.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioInfo {
    /// Duration in whole seconds.
    pub duration: Option<usize>,
    /// Audio bitrate in kbps.
    pub bitrate: Option<u32>,
    /// Sampling frequency in Hz.
    pub sample_rate: Option<u32>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub comment: Option<String>,
    pub year: Option<u32>,
    /// Track number, formatted as `n` or `n/total`.
    pub track: Option<String>,
    /// Disc number ("part of a set"), formatted as `n` or `n/total`.
    pub disc: Option<String>,
}

/// Whether `lofty` should read this file's audio metadata. The video MP4
/// container extensions (`mp4`, `m4v`, `3gp`) are intentionally excluded so
/// their duration keeps coming from the dedicated video extractor, while the
/// audio-only MP4 variants (`m4a`, `m4b`, ...) are handled here.
pub fn is_audio_ext(ext_lowercase: &str) -> bool {
    matches!(
        ext_lowercase,
        "mp3" | "mp2" | "mp1"
            | "flac"
            | "ogg" | "oga"
            | "opus"
            | "m4a" | "m4b" | "m4p" | "m4r"
            | "aac"
            | "aiff" | "aif" | "afc" | "aifc"
            | "wav" | "wave"
            | "wv"
            | "ape"
            | "mpc" | "mp+" | "mpp"
            | "spx"
    )
}

/// Format a number paired with an optional total as `n/total`, or just `n`
/// when no total is present.
fn format_numbered(value: Option<u32>, total: Option<u32>) -> Option<String> {
    value.map(|v| match total {
        Some(t) => format!("{}/{}", v, t),
        None => v.to_string(),
    })
}

/// Read audio metadata and properties for a supported audio file, or `None`
/// when the extension is not a recognized audio format or the file cannot be
/// parsed.
pub fn get_audio_info(path: &Path) -> Option<AudioInfo> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !is_audio_ext(&ext) {
        return None;
    }

    let tagged_file = read_from_path(path).ok()?;

    let properties = tagged_file.properties();
    let duration = properties.duration().as_secs();

    let mut info = AudioInfo {
        // A zero duration means lofty could not determine it; surface that as
        // an absent value rather than a misleading 0.
        duration: (duration > 0).then_some(duration as usize),
        bitrate: properties.audio_bitrate().or_else(|| properties.overall_bitrate()),
        sample_rate: properties.sample_rate(),
        ..Default::default()
    };

    if let Some(tag) = tagged_file.primary_tag().or_else(|| tagged_file.first_tag()) {
        info.title = tag.title().map(|c| c.to_string());
        info.artist = tag.artist().map(|c| c.to_string());
        info.album = tag.album().map(|c| c.to_string());
        info.genre = tag.genre().map(|c| c.to_string());
        info.comment = tag.comment().map(|c| c.to_string());
        info.year = tag.year();
        info.track = format_numbered(tag.track(), tag.track_total());
        info.disc = format_numbered(tag.disk(), tag.disk_total());
    }

    Some(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources/test/audio")
            .join(name)
    }

    #[test]
    fn test_is_audio_ext_excludes_video_mp4() {
        assert!(is_audio_ext("flac"));
        assert!(is_audio_ext("m4a"));
        assert!(is_audio_ext("mp3"));
        assert!(!is_audio_ext("mp4"));
        assert!(!is_audio_ext("m4v"));
        assert!(!is_audio_ext("mkv"));
        assert!(!is_audio_ext("txt"));
    }

    #[test]
    fn test_format_numbered() {
        assert_eq!(format_numbered(Some(4), Some(9)), Some(String::from("4/9")));
        assert_eq!(format_numbered(Some(4), None), Some(String::from("4")));
        assert_eq!(format_numbered(None, Some(9)), None);
    }

    #[test]
    fn test_get_audio_info_mp3_duration() {
        let info = get_audio_info(&fixture("silent-35s.mp3")).expect("mp3 should parse");
        assert_eq!(info.duration, Some(35));
    }

    #[test]
    fn test_get_audio_info_wav_duration() {
        let info = get_audio_info(&fixture("silent.wav")).expect("wav should parse");
        assert_eq!(info.duration, Some(15));
    }

    #[test]
    fn test_get_audio_info_rejects_non_audio() {
        assert!(get_audio_info(Path::new("nonexistent.txt")).is_none());
    }
}
