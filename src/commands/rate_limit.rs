use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{NaiveDate, Utc};
use serenity::all::UserId;

#[derive(Debug)]
pub enum RateLimitError {
    Cooldown { retry_in: Duration },
    DailyQuota,
}

#[derive(Debug, Clone, Copy)]
struct UserState {
    last_request: Instant,
    daily_count: u32,
    day_bucket: NaiveDate,
}

pub struct RateLimiter {
    inner: Mutex<HashMap<UserId, UserState>>,
    cooldown: Duration,
    daily_max: u32,
}

impl RateLimiter {
    pub fn new(cooldown: Duration, daily_max: u32) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            cooldown,
            daily_max,
        }
    }

    pub fn daily_max(&self) -> u32 {
        self.daily_max
    }

    /// Check whether `user` is allowed to make a request right now. Does NOT
    /// consume quota — call [`RateLimiter::record`] only after a successful
    /// request, so failures don't burn the user's daily allowance.
    pub fn check(&self, user: UserId) -> Result<(), RateLimitError> {
        self.check_at(user, Instant::now(), Utc::now().date_naive())
    }

    pub fn record(&self, user: UserId) {
        self.record_at(user, Instant::now(), Utc::now().date_naive());
    }

    fn check_at(&self, user: UserId, now: Instant, today: NaiveDate) -> Result<(), RateLimitError> {
        let map = self.inner.lock().expect("rate limiter mutex poisoned");
        let Some(state) = map.get(&user) else {
            return Ok(());
        };

        let elapsed = now.saturating_duration_since(state.last_request);
        if elapsed < self.cooldown {
            return Err(RateLimitError::Cooldown {
                retry_in: self.cooldown - elapsed,
            });
        }
        if state.day_bucket == today && state.daily_count >= self.daily_max {
            return Err(RateLimitError::DailyQuota);
        }
        Ok(())
    }

    fn record_at(&self, user: UserId, now: Instant, today: NaiveDate) {
        let mut map = self.inner.lock().expect("rate limiter mutex poisoned");
        let state = map.entry(user).or_insert(UserState {
            last_request: now,
            daily_count: 0,
            day_bucket: today,
        });
        if state.day_bucket != today {
            state.daily_count = 0;
            state.day_bucket = today;
        }
        state.last_request = now;
        state.daily_count += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: u64) -> UserId {
        UserId::new(id)
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn first_request_passes() {
        let rl = RateLimiter::new(Duration::from_secs(30), 20);
        assert!(rl.check(user(1)).is_ok());
    }

    #[test]
    fn cooldown_blocks_immediate_second_request() {
        let rl = RateLimiter::new(Duration::from_secs(30), 20);
        let t0 = Instant::now();
        let today = date(2026, 4, 26);
        rl.record_at(user(1), t0, today);

        match rl.check_at(user(1), t0 + Duration::from_secs(5), today) {
            Err(RateLimitError::Cooldown { retry_in }) => {
                assert!(retry_in <= Duration::from_secs(25));
                assert!(retry_in > Duration::from_secs(24));
            }
            other => panic!("expected Cooldown, got {other:?}"),
        }
    }

    #[test]
    fn cooldown_clears_after_window() {
        let rl = RateLimiter::new(Duration::from_secs(30), 20);
        let t0 = Instant::now();
        let today = date(2026, 4, 26);
        rl.record_at(user(1), t0, today);
        assert!(rl
            .check_at(user(1), t0 + Duration::from_secs(31), today)
            .is_ok());
    }

    #[test]
    fn daily_quota_blocks_after_max() {
        let rl = RateLimiter::new(Duration::from_secs(0), 3);
        let mut t = Instant::now();
        let today = date(2026, 4, 26);
        for _ in 0..3 {
            assert!(rl.check_at(user(1), t, today).is_ok());
            rl.record_at(user(1), t, today);
            t += Duration::from_secs(1);
        }
        assert!(matches!(
            rl.check_at(user(1), t, today),
            Err(RateLimitError::DailyQuota)
        ));
    }

    #[test]
    fn daily_quota_resets_at_midnight_utc() {
        let rl = RateLimiter::new(Duration::from_secs(0), 2);
        let mut t = Instant::now();
        let day1 = date(2026, 4, 26);
        for _ in 0..2 {
            rl.record_at(user(1), t, day1);
            t += Duration::from_secs(1);
        }
        assert!(matches!(
            rl.check_at(user(1), t, day1),
            Err(RateLimitError::DailyQuota)
        ));

        let day2 = date(2026, 4, 27);
        assert!(rl.check_at(user(1), t, day2).is_ok());
        rl.record_at(user(1), t, day2);
        // Counter reset to 1 — still under quota.
        assert!(rl
            .check_at(user(1), t + Duration::from_secs(1), day2)
            .is_ok());
    }

    #[test]
    fn users_are_isolated() {
        let rl = RateLimiter::new(Duration::from_secs(30), 20);
        let t = Instant::now();
        let today = date(2026, 4, 26);
        rl.record_at(user(1), t, today);
        // user 1 is in cooldown, but user 2 isn't affected
        assert!(matches!(
            rl.check_at(user(1), t + Duration::from_secs(1), today),
            Err(RateLimitError::Cooldown { .. })
        ));
        assert!(rl
            .check_at(user(2), t + Duration::from_secs(1), today)
            .is_ok());
    }

    #[test]
    fn cooldown_takes_precedence_over_daily_quota() {
        // If both apply, we surface Cooldown — clearer message ("wait Xs"
        // is more actionable than "come back tomorrow" if the user is just
        // spamming).
        let rl = RateLimiter::new(Duration::from_secs(30), 1);
        let t = Instant::now();
        let today = date(2026, 4, 26);
        rl.record_at(user(1), t, today);
        match rl.check_at(user(1), t + Duration::from_secs(5), today) {
            Err(RateLimitError::Cooldown { .. }) => {}
            other => panic!("expected Cooldown, got {other:?}"),
        }
    }
}
