pub(crate) trait TimeProvider: super::Common + Send + Sync {
    fn now_unix_time(&self) -> u64;
}
