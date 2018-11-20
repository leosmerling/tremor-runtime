// Copyright 2018, Wayfair GmbH
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use dflt;
use errors::*;
use pipeline::prelude::*;
use prometheus::IntGauge; // w/ instance
use serde_yaml;
use std::cmp::max;
use window::TimeWindow;

lazy_static! {
    static ref RATE_GAUGE: IntGauge = prom_int_gauge!("limiting_rate", "Current limiting rate.");
}

/// A Limitier algorith that just lets trough a percentage of messages
#[derive(Deserialize)]
struct Config {
    time_range: u64,
    windows: usize,
    rate: u64,
    #[serde(default = "dflt::d_0")]
    upper_limit: f64,
    #[serde(default = "dflt::d_inf")]
    lower_limit: f64,
    #[serde(default = "dflt::d_0")]
    adjustment: u64,
}

pub struct Limiter {
    window: TimeWindow,
    config: Config,
}

impl Limiter {
    pub fn new(opts: ConfValue) -> Result<Self> {
        let config: Config = serde_yaml::from_value(opts)?;

        RATE_GAUGE.set(config.rate as i64);
        Ok(Limiter {
            window: TimeWindow::new(
                config.windows,
                config.time_range / (config.windows as u64),
                config.rate,
            ),
            config,
        })
    }
}

impl Opable for Limiter {
    fn exec(&mut self, event: EventData) -> EventResult {
        match self.window.inc() {
            Ok(_) => EventResult::Next(event),
            Err(_) => EventResult::NextID(3, event),
        }
    }
    fn result(&mut self, result: EventReturn) -> EventReturn {
        match result {
            Ok(Some(f)) if f > self.config.upper_limit => {
                let m = self.window.max();
                self.window
                    .set_max(max(self.config.adjustment, m - self.config.adjustment));
                let m = self.window.max();
                RATE_GAUGE.set(m as i64);
                debug!("v {} ({})", m, f);
            }
            Ok(Some(f)) if f < self.config.lower_limit => {
                let m = self.window.max();
                let c = self.window.count();
                // Only allow max 20% buffer on growth so we do not increase
                // the maximum indefinetly
                if m < (c as f64 * 1.2) as u64 {
                    self.window.set_max(m + self.config.adjustment);
                    let m = self.window.max();
                    RATE_GAUGE.set(m as i64);
                }
                debug!("^ {} ({})", m, f);
            }
            _ => (),
        }
        result
    }
    opable_types!(ValueType::Same, ValueType::Same);
}

#[cfg(test)]
mod tests {

    use super::super::Limiter;
    use pipeline::prelude::*;
    use serde_yaml::Mapping;
    use std::iter::FromIterator;
    use std::thread::sleep;
    use std::time::Duration;
    use utils::*;

    fn conf(rate: u64) -> ConfValue {
        ConfValue::Mapping(Mapping::from_iter(
            hashmap!{
                vs("time_range") => vi(1000),
                vs("windows") => vi(100),
                vs("rate") => vi(rate),
            }.into_iter(),
        ))
    }
    #[test]
    fn no_capacity() {
        let e = EventData::new(0, 0, None, EventValue::Raw(vec![]));
        let mut c = Limiter::new("windowed", conf(0)).unwrap();
        let r = c.exec(e);

        assert_matches!(r, EventResult::NextID(3, _));
    }

    #[test]
    fn grouping_test_fail() {
        let e = EventData::new(0, 0, None, EventValue::Raw(vec![]));
        let mut c = Limiter::new("windowed", conf(1)).unwrap();
        let r = c.exec(e);

        assert_matches!(r, EventResult::Next(_));
    }

    #[test]
    fn grouping_time_refresh() {
        let e = EventData::new(0, 0, None, EventValue::Raw(vec![]));
        let mut c = Limiter::new("windowed", conf(1)).unwrap();
        let r1 = c.exec(e);
        let e = EventData::new(0, 0, None, EventValue::Raw(vec![]));
        let r2 = c.exec(e);
        // we sleep for 1.1s as this should refresh our bucket
        sleep(Duration::new(1, 200_000_000));
        let e = EventData::new(0, 0, None, EventValue::Raw(vec![]));
        let r3 = c.exec(e);
        assert_matches!(r1, EventResult::Next(_));
        assert_matches!(r2, EventResult::NextID(3, _));
        assert_matches!(r3, EventResult::Next(_));
    }
}