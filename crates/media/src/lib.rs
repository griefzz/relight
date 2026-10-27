//! What relight does with media files: renders a timeline of pieces cut from them, exports
//! it full quality or sized for Discord, and makes the thumbnails and waveforms the
//! timeline shows.

pub mod discord;
pub mod export;
pub mod previews;
pub mod timeline;

#[cfg(test)]
mod testing;
