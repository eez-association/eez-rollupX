#[cfg(test)]
mod tests {
    use hickory_resolver::{
        TokioResolver,
        config::{ConnectionConfig, NameServerConfig, ResolverConfig},
        net::runtime::TokioRuntimeProvider,
        proto::{
            op::{Message, OpCode},
            rr::{RData, Record, rdata::TXT},
            serialize::binary::{BinDecodable, BinEncodable},
        },
    };
    use reth_dns_discovery::{DnsResolver, Resolver};
    use std::{net::Ipv4Addr, time::Duration};
    use tokio::net::UdpSocket;

    async fn lookup(bad_prefix: bool, empty_question: bool, negative: bool) -> Option<String> {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = socket.local_addr().unwrap();
        let server = tokio::spawn(async move {
            loop {
                let mut bytes = [0u8; 2048];
                let (len, peer) = socket.recv_from(&mut bytes).await.unwrap();
                let request = Message::from_bytes(&bytes[..len]).unwrap();
                let mut response = Message::response(request.metadata.id, OpCode::Query);
                response.metadata.recursion_desired = request.metadata.recursion_desired;
                response.metadata.recursion_available = true;
                response.metadata.authoritative = true;
                response.queries = request.queries.clone();
                response.answers.push(Record::from_rdata(
                    request.queries[0].name().clone(),
                    60,
                    RData::TXT(TXT::new(vec!["enrtree-branch:".into(), "expected".into()])),
                ));
                if bad_prefix {
                    for mask in [1, 2, 4, 8] {
                        socket
                            .send_to(&(request.metadata.id ^ mask).to_be_bytes(), peer)
                            .await
                            .unwrap();
                    }
                }
                if empty_question {
                    let mut bad = response.clone();
                    bad.queries.clear();
                    bad.answers[0].data = RData::TXT(TXT::new(vec!["enrtree-branch:wrong".into()]));
                    socket
                        .send_to(&bad.to_bytes().unwrap(), peer)
                        .await
                        .unwrap();
                }
                if negative {
                    response.answers.clear();
                    response.metadata.response_code =
                        hickory_resolver::proto::op::ResponseCode::NXDomain;
                }
                socket
                    .send_to(&response.to_bytes().unwrap(), peer)
                    .await
                    .unwrap();
            }
        });
        let mut connection = ConnectionConfig::udp();
        connection.port = address.port();
        let config = ResolverConfig::from_parts(
            None,
            vec![],
            vec![NameServerConfig::new(address.ip(), true, vec![connection])],
        );
        let mut builder =
            TokioResolver::builder_with_config(config, TokioRuntimeProvider::default());
        builder.options_mut().timeout = Duration::from_millis(250);
        builder.options_mut().attempts = 1;
        let resolver = DnsResolver::new(builder.build().unwrap());
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            resolver.lookup_txt("probe.example."),
        )
        .await;
        server.abort();
        let _ = server.await;
        result.expect("bounded resolver lookup")
    }
    #[tokio::test]
    async fn valid_split_txt() {
        assert_eq!(
            lookup(false, false, false).await.as_deref(),
            Some("enrtree-branch:expected")
        );
    }
    #[tokio::test]
    async fn malformed_wrong_id_before_valid_response() {
        assert_eq!(
            lookup(true, false, false).await.as_deref(),
            Some("enrtree-branch:expected")
        );
    }
    #[tokio::test]
    async fn empty_question_before_valid_response() {
        assert_eq!(
            lookup(false, true, false).await.as_deref(),
            Some("enrtree-branch:expected")
        );
    }
    #[tokio::test]
    async fn negative_answer_returns_none() {
        assert!(lookup(false, false, true).await.is_none());
    }
    #[tokio::test]
    async fn silent_server_has_bounded_failure() {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = socket.local_addr().unwrap();
        let mut connection = ConnectionConfig::udp();
        connection.port = address.port();
        let config = ResolverConfig::from_parts(
            None,
            vec![],
            vec![NameServerConfig::new(address.ip(), true, vec![connection])],
        );
        let mut builder =
            TokioResolver::builder_with_config(config, TokioRuntimeProvider::default());
        builder.options_mut().timeout = Duration::from_millis(100);
        builder.options_mut().attempts = 1;
        let resolver = DnsResolver::new(builder.build().unwrap());
        let answer = tokio::time::timeout(
            Duration::from_secs(5),
            resolver.lookup_txt("probe.example."),
        )
        .await
        .expect("silent DNS server must not hang the lookup");
        assert!(answer.is_none());
        drop(socket);
    }
    #[test]
    fn invalid_resolv_conf_line_preserves_valid_server() {
        let (config, _) = hickory_resolver::system_conf::parse_resolv_conf(
            b"nameserver 127.0.0.1\nnameserver not-an-ip\n",
        )
        .expect("usable lines retained");
        assert_eq!(config.name_servers().len(), 1);
    }
}
