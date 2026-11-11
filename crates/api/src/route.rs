use reqwest::Method;

/// Rate-limit identity of a request. Discord scopes limits per bucket *and* top-level
/// ("major") parameter, so the key keeps channel/guild/webhook ids and erases the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Route {
    pub key: String,
    pub major: String,
}

impl Route {
    pub fn new(method: &Method, path: &str) -> Self {
        let path = path.split('?').next().unwrap_or_default();
        let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let mut key = String::with_capacity(path.len() + 8);
        key.push_str(method.as_str());
        key.push(' ');
        let mut major = String::new();
        let mut prev = "";
        for (i, seg) in segs.iter().enumerate() {
            key.push('/');
            let is_id = !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit());
            let part: &str = if i == 1 && is_id && matches!(prev, "channels" | "guilds" | "webhooks") {
                major = format!("{prev}/{seg}");
                seg
            } else if i == 2 && segs[0] == "webhooks" {
                // Webhook token is part of the major parameter.
                major.push('/');
                major.push_str(seg);
                seg
            } else if prev == "reactions" {
                ":emoji"
            } else if is_id {
                ":id"
            } else {
                seg
            };
            key.push_str(part);
            prev = seg;
        }
        Route { key, major }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_major_and_erases_minor_ids() {
        let r = Route::new(&Method::GET, "/channels/123/messages/456");
        assert_eq!(r.key, "GET /channels/123/messages/:id");
        assert_eq!(r.major, "channels/123");

        let r = Route::new(&Method::PUT, "/channels/1/messages/2/reactions/%F0%9F%91%8D/@me");
        assert_eq!(r.key, "PUT /channels/1/messages/:id/reactions/:emoji/@me");

        let r = Route::new(&Method::GET, "/guilds/9/members?limit=5");
        assert_eq!(r.key, "GET /guilds/9/members");
        assert_eq!(r.major, "guilds/9");

        let r = Route::new(&Method::GET, "/users/@me/guilds/77/member");
        assert_eq!(r.key, "GET /users/@me/guilds/:id/member");
        assert_eq!(r.major, "");

        let r = Route::new(&Method::POST, "/webhooks/5/tok3n");
        assert_eq!(r.major, "webhooks/5/tok3n");
    }
}
