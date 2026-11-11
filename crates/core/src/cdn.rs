//! Discord CDN URL builders. Size parameters keep downloads proportional to what we display.

use crate::id::Id;

pub const CDN: &str = "https://cdn.discordapp.com";
pub const MEDIA: &str = "https://media.discordapp.net";

/// Discord only accepts power-of-two sizes between 16 and 4096.
pub fn clamp_size(size: u32) -> u32 {
    size.clamp(16, 4096).next_power_of_two().min(4096)
}

pub fn user_avatar(user: Id, hash: &str, size: u32) -> String {
    format!("{CDN}/avatars/{user}/{hash}.png?size={}", clamp_size(size))
}

pub fn default_avatar(user: Id, discriminator: &str) -> String {
    let idx = match discriminator.parse::<u64>() {
        Ok(d) if d != 0 => d % 5,
        _ => (user.0 >> 22) % 6,
    };
    format!("{CDN}/embed/avatars/{idx}.png")
}

pub fn guild_icon(guild: Id, hash: &str, size: u32) -> String {
    format!("{CDN}/icons/{guild}/{hash}.png?size={}", clamp_size(size))
}

pub fn emoji(id: Id, _animated: bool, size: u32) -> String {
    // Static PNG even for animated emoji: the client renders the first frame only, which
    // keeps idle CPU/GPU at zero.
    format!("{CDN}/emojis/{id}.png?size={}", clamp_size(size))
}

pub fn sticker(id: Id, format_type: u8, size: u32) -> String {
    let ext = if format_type == 4 { "gif" } else { "png" };
    format!("{MEDIA}/stickers/{id}.{ext}?size={}", clamp_size(size))
}

/// Append media-proxy `width`/`height` params so the server downsizes previews.
pub fn with_size_params(url: &str, w: Option<u32>, h: Option<u32>, max_w: u32, max_h: u32) -> String {
    let (Some(w), Some(h)) = (w, h) else { return url.to_string() };
    if w == 0 || h == 0 {
        return url.to_string();
    }
    let scale = (max_w as f64 / w as f64).min(max_h as f64 / h as f64).min(1.0);
    let (tw, th) = (((w as f64) * scale).round().max(1.0) as u32, ((h as f64) * scale).round().max(1.0) as u32);
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}width={tw}&height={th}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_powers_of_two() {
        assert_eq!(clamp_size(40), 64);
        assert_eq!(clamp_size(1), 16);
        assert_eq!(clamp_size(10_000), 4096);
    }

    #[test]
    fn default_avatar_index() {
        assert!(default_avatar(Id(175_928_847_299_117_063), "0").ends_with("/embed/avatars/2.png"));
        assert!(default_avatar(Id(1), "1337").ends_with("/embed/avatars/2.png"));
    }

    #[test]
    fn preview_params() {
        let u = with_size_params("https://media.discordapp.net/a.png?ex=1", Some(2000), Some(1000), 400, 300);
        assert_eq!(u, "https://media.discordapp.net/a.png?ex=1&width=400&height=200");
        assert_eq!(with_size_params("x", None, Some(1), 1, 1), "x");
    }
}
