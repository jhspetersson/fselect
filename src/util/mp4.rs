use std::fs::File;
use std::io;
use std::io::BufReader;
use std::path::Path;

use crate::util::{Dimensions, Duration};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mp4Info {
    pub dimensions: Option<Dimensions>,
    pub duration: Option<Duration>,
}

pub fn is_mp4_ext(ext_lowercase: &str) -> bool {
    // All ISO-BMFF containers mp4parse reads; util/audio.rs excludes
    // these from lofty on the assumption they are handled here.
    matches!(ext_lowercase, "mp4" | "m4v" | "mov" | "3gp")
}

pub fn get_mp4_info(path: &Path) -> Option<Mp4Info> {
    let extension = path.extension()?.to_str()?.to_lowercase();
    is_mp4_ext(&extension).then(|| read_mp4_info(path).unwrap_or_default())
}

pub fn read_mp4_info(path: &Path) -> io::Result<Mp4Info> {
    let fd = File::open(path)?;
    let mut reader = BufReader::new(fd);
    let context = mp4parse::read_mp4(&mut reader)?;
    let dimensions = context
        .tracks
        .iter()
        .find(|track| track.track_type == mp4parse::TrackType::Video)
        .and_then(|track| {
            track.tkhd.as_ref().map(|tkhd| Dimensions {
                width: (tkhd.width / 65536) as usize,
                height: (tkhd.height / 65536) as usize,
            })
        });
    // The track header duration is expressed in movie timescale units
    // (often 1000, but 600 for QuickTime-origin files).
    let timescale = context
        .timescale
        .map(|ts| ts.0)
        .filter(|&ts| ts > 0)
        .unwrap_or(1000);
    let duration = context
        .tracks
        .iter()
        .filter_map(|track| track.tkhd.as_ref())
        // Fragmented/DASH files store all-ones for "unknown" (ISO
        // 14496-12); reporting it would show a ~4-million-second video.
        .filter(|tkhd| tkhd.duration != u64::MAX && tkhd.duration != u32::MAX as u64)
        .map(|tkhd| tkhd.duration)
        .max()
        .map(|duration| Duration {
            length: (duration / timescale) as usize,
        });
    Ok(Mp4Info { dimensions, duration })
}
