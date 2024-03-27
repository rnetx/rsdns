use hickory_proto::{
    op::{Message, MessageType, ResponseCode},
    rr::{rdata::SOA, RData, Record, RecordType},
};

// Please Check Query Is Valid!!!
pub(crate) fn generate_empty_message(request: &Message) -> Message {
    let query = request.query();
    let mut answer = Record::with(query.unwrap().name().clone(), RecordType::SOA, 10);
    answer.set_data(Some(RData::SOA(SOA::new(
        "fake-ns.rsdns.".parse().unwrap(),
        "fake-mbox.rsdns.".parse().unwrap(),
        2024010100,
        1800,
        900,
        604800,
        86400,
    ))));
    let mut response = Message::new();
    response.set_id(request.id());
    response.set_message_type(MessageType::Response);
    response.set_op_code(request.op_code());
    response.add_query(query.unwrap().clone());
    response.add_name_server(answer);
    response.set_response_code(ResponseCode::NoError);
    response
}
