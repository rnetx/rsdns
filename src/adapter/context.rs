use std::{collections::HashMap, net::IpAddr, sync::Arc};

use hickory_proto::op::Message;

use crate::log;

#[derive(Clone)]
pub(crate) struct Context {
    log_tracker: Arc<log::Tracker>,
    listener: String,
    client_ip: IpAddr,
    request: Message,
    response: Option<Message>,
    mark: i16,
    metadata: HashMap<String, String>,
}

impl Context {
    pub(crate) fn new(request: Message, listener: String, client_ip: IpAddr) -> Self {
        Self {
            log_tracker: Arc::new(log::Tracker::default()),
            listener,
            client_ip,
            request,
            response: None,
            mark: 0,
            metadata: HashMap::new(),
        }
    }

    pub(crate) fn log_tracker(&self) -> &Arc<log::Tracker> {
        &self.log_tracker
    }

    pub(crate) fn client_ip(&self) -> &IpAddr {
        &self.client_ip
    }

    pub(crate) fn request(&self) -> &Message {
        &self.request
    }

    pub(crate) fn request_mut(&mut self) -> &mut Message {
        &mut self.request
    }

    pub(crate) fn response(&self) -> Option<&Message> {
        self.response.as_ref()
    }

    pub(crate) fn response_mut(&mut self) -> Option<&mut Message> {
        self.response.as_mut()
    }

    pub(crate) fn take_response(&mut self) -> Option<Message> {
        self.response.take()
    }

    pub(crate) fn replace_response(&mut self, new_response: Message) -> Option<Message> {
        self.response.replace(new_response)
    }

    pub(crate) fn listener(&self) -> &str {
        &self.listener
    }

    pub(crate) fn mark(&self) -> i16 {
        self.mark
    }

    pub(crate) fn set_mark(&mut self, mark: i16) {
        self.mark = mark;
    }

    pub(crate) fn metadata(&self) -> &HashMap<String, String> {
        &self.metadata
    }

    pub(crate) fn metadata_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.metadata
    }
}

impl AsRef<log::Tracker> for Context {
    fn as_ref(&self) -> &log::Tracker {
        &self.log_tracker
    }
}
