
use std::marker::PhantomData;

use coarsetime as time;


pub struct CallRateLimiter<T, F>
where F: FnMut(T)
{
    interval: time::Duration,
    last_call: Option<time::Instant>,
    param_type: PhantomData<T>,
    min_call_count: u64,
    call_counter: u64,
    f: F,
}

impl<T, F> CallRateLimiter<T, F>
where F: FnMut(T)
{

    /// interval in seconds
    pub fn new(interval: impl Into<f64>, f: F) -> Self {
        let interval = interval.into();
        if interval.is_sign_negative() { panic!("invalid interval: {interval}") }
        let interval = time::Duration::new(interval.trunc() as u64, (interval.fract() * 1_000_000_000_f64) as u32);
        Self { interval, last_call: None, min_call_count: 0, call_counter: 0, param_type: PhantomData, f }
    }

    pub fn call(&mut self, param: T) {
        self.call_counter += 1;
        if self.call_counter > self.min_call_count {
            let should_call = match self.last_call {
                Some(last_call) => {
                    let elapsed = last_call.elapsed();
                    self.min_call_count = if elapsed > self.interval {
                        self.min_call_count.saturating_add(1)
                    } else {
                        self.min_call_count.saturating_sub(1)
                    };
                    elapsed >= self.interval
                }
                None => true
            };
            if should_call {
                self.last_call = Some(time::Instant::now());
                (self.f)(param);
                self.call_counter = 0;
            }
        }
    }

    pub fn call_unconditional(&mut self, param: T) {
        (self.f)(param);
    }

}