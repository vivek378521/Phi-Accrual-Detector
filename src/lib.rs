use std::error::Error;
use std::ops::Sub;
use std::sync::{Arc};
use tokio::sync::{RwLock, RwLockReadGuard};
use async_trait::async_trait;
use libm::{erf, log10};
use chrono::{DateTime, Local};

#[derive(Clone, Debug)]
pub struct Statistics {
    arrival_intervals: Vec<u64>,
    last_arrived_at: DateTime<Local>,
    window_length: u32,
    n: u32,
}

#[derive(Debug)]
pub struct Detector {
    statistics: RwLock<Statistics>,
}

impl Detector {
    pub fn new(window_length: u32) -> Self {
        Detector {
            statistics: RwLock::new(Statistics::new(window_length)),
        }
    }
}

impl Statistics {
    pub fn new(window_length: u32) -> Self {
        Self {
            arrival_intervals: vec![],
            last_arrived_at: Local::now(),
            window_length,
            n: 0,
        }
    }

    pub fn insert(&mut self, arrived_at: DateTime<Local>) {

        // insert first element
        if self.n == 0 {
            self.last_arrived_at = arrived_at;
            self.n += 1;
            return;
        }


        if self.n - 1 == self.window_length {
            self.arrival_intervals.remove(0);
            self.n -= 1;
        }
        if self.n != 0 {
            let arrival_interval = arrived_at.sub(self.last_arrived_at).num_milliseconds() as u64;
            self.arrival_intervals.push(arrival_interval);
        }
        self.last_arrived_at = arrived_at;
        self.n += 1;
    }
}

#[async_trait]
trait PhiCore {
    async fn mean_with_stats<'a>(&self, stats: Arc<RwLockReadGuard<'a, Statistics>>) -> Result<f64, Box<dyn Error>>;
    async fn variance_and_mean(&self) -> Result<(f64, f64), Box<dyn Error>>;
}

#[async_trait]
pub trait PhiInteraction {
    async fn insert(&self, arrived_at: DateTime<Local>) -> Result<(), Box<dyn Error>>;
    async fn phi(&self, t: DateTime<Local>) -> Result<f64, Box<dyn Error>>;
    async fn last_arrived_at(&self) -> Result<DateTime<Local>, Box<dyn Error>>;
}

#[async_trait]
impl PhiCore for Detector {
    async fn mean_with_stats<'a>(&self, stats: Arc<RwLockReadGuard<'a, Statistics>>) -> Result<f64, Box<dyn Error>> {
        let mut mean: f64 = 0.;
        let len = &stats.arrival_intervals.len();
        for v in &stats.arrival_intervals {
            mean += *v as f64 / *len as f64;
        }
        Ok(mean)
    }

    async fn variance_and_mean(&self) -> Result<(f64, f64), Box<dyn Error>> {
        let mut variance: f64 = 0.;
        let stats = Arc::new(self.statistics.read().await);
        let mu = self.mean_with_stats(Arc::clone(&stats)).await?;
        let len = &stats.arrival_intervals.len();
        for v in &stats.arrival_intervals {
            let val = ((*v as f64 - mu) * (*v as f64 - mu)) / *len as f64;
            variance += val;
        }
        Ok((variance, mu))
    }
}

fn normal_cdf(t: f64, mu: f64, sigma: f64) -> f64 {

    if sigma == 0. {
        return if t == mu {
            1.
        } else {
            0.
        };
    }

    let z = (t - mu) / sigma;
    0.5 + 0.5 * (erf(z))
}

#[async_trait]
impl PhiInteraction for Detector {
    async fn insert(&self, arrived_at: DateTime<Local>) -> Result<(), Box<dyn Error>> {
        let mut stats = self.statistics.write().await;
        stats.insert(arrived_at);
        Ok(())
    }

    async fn phi(&self, t: DateTime<Local>) -> Result<f64, Box<dyn Error>> {
        let (sigma_sq, mu) = self.variance_and_mean().await?;
        let sigma = sigma_sq.sqrt();
        let last_arrived_at = self.last_arrived_at().await?;
        let ft = normal_cdf(t.sub(last_arrived_at).num_milliseconds() as f64, mu, sigma);
        let phi = -log10(1. - ft);
        Ok(phi)
    }

    async fn last_arrived_at(&self) -> Result<DateTime<Local>, Box<dyn Error>> {
        Ok(self.statistics.read().await.last_arrived_at)
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Add;
    use chrono::{Duration, Local};
    use tokio::sync::RwLock;
    use crate::{Detector, PhiCore, PhiInteraction, Statistics};

    #[tokio::test]
    async fn test_variant_mean_and_variance_combo_calculation() {
        let mut stats = Statistics::new(10);
        let mut i = 0;
        let mut curr_time = Local::now();
        &stats.insert(curr_time.clone());
        let expect_vals = [1630, 4421, 1514, 216, 231, 931, 4182, 102, 104, 241, 5132];
        while i < expect_vals.len() {
            curr_time = curr_time.add(Duration::milliseconds(expect_vals[i]));
            let arrived_at = curr_time;
            &stats.insert(arrived_at);
            i += 1;
        }
        let detector = Detector {
            statistics: RwLock::new(stats),
        };
        let (mut variance, mut mean) = detector.variance_and_mean().await.unwrap();
        mean = (mean * 100.0).round() * 0.01;
        variance = (variance * 100.0).round() * 0.01;
        assert_eq!(1707.4, mean);
        assert_eq!(3755791.64, variance);

        let mut suspicion_level: Vec<f64> = vec![];
        for i in 1..10 {
            curr_time = curr_time.add(Duration::milliseconds(250));
            suspicion_level.push(detector.phi(curr_time).await.unwrap())
        }
        println!("suspicion -> {:?}", suspicion_level);
        for i in 1..suspicion_level.len() {
            assert!(suspicion_level[i] > suspicion_level[i - 1]);
        }
    }


    #[tokio::test]
    async fn test_constant_phi_with_constant_pings_calculation() {
        let stats = Statistics::new(10);
        let detector = Detector {
            statistics: RwLock::new(stats),
        };
        let mut i = 0;
        let mut curr_time = Local::now();
        while i <= 100 {
            let arrived_at = curr_time;
            &detector.insert(arrived_at).await;
            curr_time = curr_time.add(Duration::milliseconds(10));
            i += 10;
        }
        let (mut variance, mut mean) = detector.variance_and_mean().await.unwrap();
        mean = (mean * 100.0).round() * 0.01;
        variance = (variance * 100.0).round() * 0.01;
        assert_eq!(10., mean);
        assert_eq!(0., variance);
        curr_time = curr_time.add(Duration::milliseconds(10));
        assert_eq!(0., detector.phi(curr_time).await.unwrap());
    }
}
