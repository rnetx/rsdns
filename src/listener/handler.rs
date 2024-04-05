use std::{net::IpAddr, sync::Arc, time::Duration};

use hickory_proto::op::Message;

use crate::{adapter, common, error, info, log, upstream};

pub(super) async fn handle(
    workflow: Arc<Box<dyn adapter::Workflow>>,
    logger: Arc<Box<dyn log::Logger>>,
    listener_tag: String,
    query_timeout: Option<Duration>,
    peer_addr: IpAddr,
    request: Message,
) -> Option<Message> {
    let mut ctx = match adapter::Context::new(request, listener_tag, peer_addr.clone()) {
        Ok(v) => v,
        Err(e) => {
            error!(
                logger,
                "invalid request message failed: {}, peer_addr: {}", e, peer_addr,
            );
            return None;
        }
    };
    let request_info = upstream::show_query(ctx.request());
    info!(
        logger,
        { tracker = ctx.log_tracker() },
        "query: {}, peer_addr: {}",
        request_info,
        peer_addr,
    );
    let execute_fut = workflow.execute(&mut ctx);
    let res = match query_timeout {
        Some(timeout) => match tokio::time::timeout(timeout, execute_fut).await {
            Ok(res) => res,
            Err(e) => {
                error!(
                    logger,
                    { tracker = ctx.log_tracker() },
                    "query timeout: {}",
                    e
                );
                return None;
            }
        },
        None => execute_fut.await,
    };
    if let Err(e) = res {
        error!(
            logger,
            { tracker = ctx.log_tracker() },
            "workflow [{}] execute failed: {}, peer_addr: {}",
            workflow.tag(),
            e,
            peer_addr,
        );
        return None;
    }
    let response = match ctx.take_response() {
        Some(response) => {
            info!(logger, { tracker = ctx.log_tracker() }, "query success");
            response
        }
        None => {
            info!(
                logger,
                { tracker = ctx.log_tracker() },
                "empty response, generate a empty response"
            );
            common::generate_empty_message(ctx.request())
        }
    };
    Some(response)
}
