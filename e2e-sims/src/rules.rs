//! Fault-injection rules: pure matching/bookkeeping, no transport concerns.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    /// Omitted = default Matcher = matches everything.
    #[serde(default)]
    pub matcher: Matcher,
    pub action: Action,
    /// None = always; Some(n) = fires n more times then expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub times: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Matcher {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_glob: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_contains: Option<String>,
    /// 1-based, against this sim's total request count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nth_call: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    #[serde(rename_all = "camelCase")]
    Status {
        code: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<serde_json::Value>,
    },
    /// Delay, then passthrough.
    #[serde(rename_all = "camelCase")]
    Delay {
        ms: u64,
    },
    Stall,
    #[serde(rename_all = "camelCase")]
    Drop {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after_bytes: Option<usize>,
    },
    /// Fraction of the real response body to keep.
    #[serde(rename_all = "camelCase")]
    Truncate {
        fraction: f32,
    },
    #[serde(rename_all = "camelCase")]
    Corrupt {
        mode: CorruptMode,
    },
    Passthrough,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorruptMode {
    MalformedJson,
    WrongContentType,
    Garbage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleSet {
    rules: Vec<Rule>,
    calls: u64,
}

impl RuleSet {
    pub fn push(&mut self, r: Rule) {
        self.rules.push(r);
    }

    /// Rules only; the call counter is sim-lifetime state.
    pub fn clear(&mut self) {
        self.rules.clear();
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn calls(&self) -> u64 {
        self.calls
    }

    /// Increments the call counter, then returns the first matching non-expired
    /// rule's action, decrementing its `times` and removing it when exhausted.
    pub fn decide(&mut self, method: &str, path: &str, body: &[u8]) -> Action {
        self.calls += 1;
        let n = self.calls;
        let Some(i) = self
            .rules
            .iter()
            .position(|r| r.matcher.matches(method, path, body, n))
        else {
            return Action::Passthrough;
        };
        let action = self.rules[i].action.clone();
        if let Some(t) = self.rules[i].times.as_mut() {
            *t = t.saturating_sub(1);
            if *t == 0 {
                self.rules.remove(i);
            }
        }
        action
    }
}

impl Matcher {
    fn matches(&self, method: &str, path: &str, body: &[u8], nth: u64) -> bool {
        if let Some(m) = &self.method
            && !m.eq_ignore_ascii_case(method)
        {
            return false;
        }
        if let Some(g) = &self.path_glob
            && !glob_match(g, path)
        {
            return false;
        }
        if let Some(needle) = &self.body_contains
            && !contains_sub(body, needle.as_bytes())
        {
            return false;
        }
        if let Some(want) = self.nth_call
            && want != nth
        {
            return false;
        }
        true
    }
}

fn contains_sub(hay: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || hay.windows(needle.len()).any(|w| w == needle)
}

/// `*` matches any run of characters, path separators included.
fn glob_match(pattern: &str, path: &str) -> bool {
    let mut parts = pattern.split('*');
    let Some(first) = parts.next() else {
        return false;
    };
    let Some(mut rest) = path.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, mids)) = parts.split_last() else {
        return rest.is_empty();
    };
    for m in mids {
        match rest.find(m) {
            Some(i) => rest = &rest[i + m.len()..],
            None => return false,
        }
    }
    // Trailing "*" yields an empty last piece, which any suffix satisfies.
    rest.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn status_rule(m: Matcher, times: Option<u32>) -> Rule {
        Rule {
            matcher: m,
            action: Action::Status {
                code: 503,
                body: None,
            },
            times,
        }
    }
    #[test]
    fn matcher_omitted_matches_everything() {
        let r: Rule = serde_json::from_str(r#"{"action":{"type":"status","code":503}}"#).unwrap();
        assert!(r.matcher.method.is_none() && r.matcher.path_glob.is_none());
        let mut rs = RuleSet::default();
        rs.push(r);
        assert!(matches!(
            rs.decide("GET", "/anything", b""),
            Action::Status { code: 503, .. }
        ));
    }
    #[test]
    fn empty_ruleset_passthrough() {
        let mut rs = RuleSet::default();
        assert!(matches!(rs.decide("GET", "/x", b""), Action::Passthrough));
    }
    #[test]
    fn glob_and_method_match() {
        let mut rs = RuleSet::default();
        rs.push(status_rule(
            Matcher {
                method: Some("post".into()),
                path_glob: Some("/v1beta/*".into()),
                body_contains: None,
                nth_call: None,
            },
            None,
        ));
        assert!(matches!(
            rs.decide("POST", "/v1beta/models/g:generateContent", b""),
            Action::Status { .. }
        ));
        assert!(matches!(
            rs.decide("GET", "/v1beta/x", b""),
            Action::Passthrough
        ));
        assert!(matches!(
            rs.decide("POST", "/api/get", b""),
            Action::Passthrough
        ));
    }
    #[test]
    fn times_expires() {
        let mut rs = RuleSet::default();
        rs.push(status_rule(Matcher::default(), Some(2)));
        assert!(matches!(rs.decide("GET", "/", b""), Action::Status { .. }));
        assert!(matches!(rs.decide("GET", "/", b""), Action::Status { .. }));
        assert!(matches!(rs.decide("GET", "/", b""), Action::Passthrough)); // fail-twice-then-succeed
    }
    #[test]
    fn nth_call_and_body_contains() {
        let mut rs = RuleSet::default();
        rs.push(status_rule(
            Matcher {
                nth_call: Some(2),
                ..Default::default()
            },
            None,
        ));
        assert!(matches!(rs.decide("GET", "/", b""), Action::Passthrough)); // call 1
        assert!(matches!(rs.decide("GET", "/", b""), Action::Status { .. })); // call 2
        let mut rs2 = RuleSet::default();
        rs2.push(status_rule(
            Matcher {
                body_contains: Some("needle".into()),
                ..Default::default()
            },
            None,
        ));
        assert!(matches!(
            rs2.decide("POST", "/", b"hay needle stack"),
            Action::Status { .. }
        ));
        assert!(matches!(
            rs2.decide("POST", "/", b"hay"),
            Action::Passthrough
        ));
    }
    #[test]
    fn first_match_wins_in_insertion_order() {
        let mut rs = RuleSet::default();
        rs.push(Rule {
            matcher: Matcher::default(),
            action: Action::Delay { ms: 5 },
            times: None,
        });
        rs.push(status_rule(Matcher::default(), None));
        assert!(matches!(rs.decide("GET", "/", b""), Action::Delay { .. }));
    }

    #[test]
    fn rule_json_round_trip() {
        let json = r#"{"matcher":{"method":"POST","pathGlob":"/v1beta/*","nthCall":2},"action":{"type":"status","code":503},"times":2}"#;
        let r: Rule = serde_json::from_str(json).unwrap();
        assert_eq!(r.matcher.path_glob.as_deref(), Some("/v1beta/*"));
        assert_eq!(r.matcher.nth_call, Some(2));
        assert_eq!(r.times, Some(2));
        assert!(matches!(
            r.action,
            Action::Status {
                code: 503,
                body: None
            }
        ));
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        assert_eq!(v["matcher"]["pathGlob"], "/v1beta/*");
        assert_eq!(v["action"]["type"], "status");
    }

    #[test]
    fn drop_after_bytes_round_trip() {
        let json = r#"{"matcher":{},"action":{"type":"drop","afterBytes":10}}"#;
        let r: Rule = serde_json::from_str(json).unwrap();
        assert!(matches!(
            r.action,
            Action::Drop {
                after_bytes: Some(10)
            }
        ));
        assert_eq!(r.times, None);
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        assert_eq!(v["action"]["afterBytes"], 10);
    }

    #[test]
    fn corrupt_mode_wire_names() {
        let v = serde_json::to_value(Action::Corrupt {
            mode: CorruptMode::MalformedJson,
        })
        .unwrap();
        assert_eq!(v["type"], "corrupt");
        assert_eq!(v["mode"], "malformed_json");
    }
}
