use serde_json::Value;

use super::{CacheWarning, UsageTotals};

#[derive(Debug, PartialEq, Eq)]
struct Prefix {
    segment: u64,
    digest: String,
    service_tier: Option<String>,
}

#[derive(Debug, Default)]
pub(super) struct TurnCache {
    prepared: bool,
    prefix: Option<Prefix>,
}

impl TurnCache {
    pub(super) fn prepare(&mut self, value: &Value) {
        let prefix = (|| {
            Some(Prefix {
                segment: value["segment"].as_u64()?,
                digest: value["prefix_hash"].as_str()?.to_owned(),
                service_tier: value["settings"]["service_tier"]
                    .as_str()
                    .map(str::to_owned),
            })
        })();
        if !self.prepared {
            self.prepared = true;
            self.prefix = prefix;
        } else if self.prefix != prefix {
            self.prefix = None;
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct CacheHistory {
    prefix: Option<Prefix>,
    warmed: u8,
    last_low: Option<(u64, u64)>,
    warning: Option<CacheWarning>,
}

impl CacheHistory {
    pub(super) fn warning(&self) -> Option<CacheWarning> {
        self.warning.clone()
    }

    pub(super) fn observe_prefix(&mut self, turn: &TurnCache) {
        if self.prefix != turn.prefix {
            *self = Self::default();
        }
    }

    pub(super) fn finish(&mut self, turn: u64, cache: TurnCache, usage: &UsageTotals) -> bool {
        if self.prefix != cache.prefix {
            *self = Self {
                prefix: cache.prefix,
                ..Self::default()
            };
        }
        let measured = usage.cache_measured_input_tokens;
        let read = usage.cache_measured_read_tokens;
        if self.prefix.is_none() || measured == 0 || read > measured {
            self.last_low = None;
            self.warning = None;
            return false;
        }
        if self.warmed < 2 {
            self.warmed += 1;
            return false;
        }
        if read >= measured.div_ceil(2) {
            self.last_low = None;
            self.warning = None;
            return false;
        }
        if let Some((previous_read, previous_input)) = self.last_low.replace((read, measured)) {
            self.warning = Some(CacheWarning {
                since_turn: self
                    .warning
                    .as_ref()
                    .map_or(turn, |warning| warning.since_turn),
                latest_percent: (u128::from(read) * 100 / u128::from(measured)) as u8,
                read_tokens: previous_read.saturating_add(read),
                input_tokens: previous_input.saturating_add(measured),
            });
        }
        true
    }
}
