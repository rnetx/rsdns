use chrono::{DateTime, Local};
use rand::Rng;

pub(crate) struct Tracker {
    id: String,
    init_time: DateTime<Local>,
}

impl Default for Tracker {
    fn default() -> Self {
        let id: u64 = rand::thread_rng().gen_range(10000000..99999999);
        Self {
            id: id.to_string(),
            init_time: Local::now(),
        }
    }
}

impl Tracker {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn duration(&self) -> std::time::Duration {
        Local::now()
            .signed_duration_since(self.init_time)
            .to_std()
            .unwrap()
    }
}
