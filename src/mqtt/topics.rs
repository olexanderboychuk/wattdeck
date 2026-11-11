/// Topic kinds on the developer ("open") path. Verified from the official docs table
/// and from MichelFR `const.py`: `quota | set | set_reply | get | get_reply | status`.
pub const KINDS: [&str; 6] = ["get", "get_reply", "set", "set_reply", "quota", "status"];

/// The developer path. `[DOC]` + `[MULTI]`: `/open/{certificateAccount}/{sn}/{kind}`.
pub fn open_topic(account: &str, sn: &str, kind: &str) -> String {
    format!("/open/{account}/{sn}/{kind}")
}

/// The app path. `[MULTI]`: `/app/{userId}/{sn}/thing/property/{kind}`.
pub fn app_topic(user: &str, sn: &str, kind: &str) -> String {
    format!("/app/{user}/{sn}/thing/property/{kind}")
}

/// Topics to subscribe, in the order they must be sent.
///
/// Verified live against `mqtt-e.ecoflow.com` on 2026-11-10 with this account's
/// developer key: `quota`, `status` and `set_reply` get SUBACK success, `get_reply`
/// is refused (SUBACK 0x80). EcoFlow also refuses `SUBSCRIBE /app/device/property/<sn>`
/// (SUBACK 128) as of 2026-07-26, and a combined multi-topic SUBSCRIBE fails, so each
/// topic goes out as its own packet. `quota` carries the partial blocks, `status` the
/// online flag, `set_reply` the command acknowledgements; the full property map comes
/// from the REST endpoint, which is the backfill.
pub fn subscribe_topics(account: &str, sn: &str) -> Vec<String> {
    [
        open_topic(account, sn, "quota"),
        open_topic(account, sn, "status"),
        open_topic(account, sn, "set_reply"),
    ]
    .to_vec()
}

/// The publish topic for a set command. `[DOC]` + `[MULTI]`:
/// `/open/{certificateAccount}/{sn}/set`. The device answers on `set_reply`.
pub fn set_topic(account: &str, sn: &str) -> String {
    open_topic(account, sn, "set")
}

/// The publish topic for `latestQuotas`, which keeps the device pushing. Devices slow
/// their cadence when nobody polls, so this is sent every 20–60 s.
pub fn refresh_topic(account: &str, sn: &str) -> String {
    open_topic(account, sn, "get")
}

/// Which serial a topic belongs to. The serial is the second-to-last path element.
pub fn sn_of(account: &str, topic: &str) -> Option<String> {
    let rest = topic.strip_prefix(&format!("/open/{account}/"))?;
    let sn = rest.split('/').next()?;
    if sn.is_empty() {
        None
    } else {
        Some(sn.to_string())
    }
}

/// The kind suffix of a topic.
pub fn kind_of(topic: &str) -> Option<&'static str> {
    let last = topic.split('/').next_back()?;
    KINDS.into_iter().find(|k| *k == last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_path_is_as_documented() {
        assert_eq!(
            open_topic("open-testacct", "R351ZEB5PGC90303", "get_reply"),
            "/open/open-testacct/R351ZEB5PGC90303/get_reply"
        );
    }

    #[test]
    fn app_path_is_the_app_variant() {
        assert_eq!(
            app_topic("u1", "SN1", "set_reply"),
            "/app/u1/SN1/thing/property/set_reply"
        );
    }

    #[test]
    fn subscribes_are_ordered_with_the_snapshot_first() {
        let t = subscribe_topics("acct", "SN1");
        assert_eq!(t.len(), 3);
        assert!(t[0].ends_with("quota"));
        assert!(t[1].ends_with("status"));
        assert!(t[2].ends_with("set_reply"));
        // The device-property topic is not subscribed: EcoFlow refuses it (SUBACK 128).
        assert!(!t.iter().any(|x| x.contains("device/property")));
        // `get_reply` is refused live, so it is not subscribed.
        assert!(!t.iter().any(|x| x.ends_with("get_reply")));
    }

    #[test]
    fn serial_and_kind_are_read_back_from_a_topic() {
        let t = "/open/acct/R351ZEB5PGC90303/quota";
        assert_eq!(sn_of("acct", t).as_deref(), Some("R351ZEB5PGC90303"));
        assert_eq!(kind_of(t), Some("quota"));
        assert_eq!(kind_of("/open/acct/SN1/unknown"), None);
        assert_eq!(sn_of("other", t), None);
    }

    #[test]
    fn the_refresh_publish_target_is_the_get_topic() {
        assert_eq!(refresh_topic("acct", "SN1"), "/open/acct/SN1/get");
    }
}
