pub(crate) trait Listener: super::Common + Send + Sync {
    fn tag(&self) -> &str;
    fn r#type(&self) -> &str;
}
