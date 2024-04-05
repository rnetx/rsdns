use std::{
    io,
    net::{IpAddr, Ipv4Addr},
};

use hickory_proto::op::{Message, Query};

use crate::fatal;

use super::*;

#[test]
fn test_macro() {
    let basic =
        BasicLogger::new(false, super::Level::Info, false, Box::new(io::stdout())).into_box();

    let tracker = super::Tracker::default();
    let ctx = crate::adapter::Context::new(
        {
            let mut msg = Message::new();
            msg.add_query(Query::new());
            msg
        },
        "".to_owned(),
        IpAddr::V4(Ipv4Addr::LOCALHOST),
    )
    .unwrap();
    let ctx_ref = Some(&ctx);
    fatal!(basic, "test");
    fatal!(basic, tracker, "test");
    fatal!(basic, { tracker = tracker }, "test");
    fatal!(
        basic,
        { option_tracker = ctx_ref.map(|w| w.log_tracker().as_ref()) },
        "test"
    );
}
