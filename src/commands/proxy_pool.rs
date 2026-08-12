pub async fn run(
    listen: String,
    cidr: Option<String>,
    source_ips: Option<String>,
) -> anyhow::Result<()> {
    let listen_addr = listen.parse()?;
    let ips: Vec<std::net::IpAddr> = if let Some(c) = cidr {
        webfind::engine::proxy_server::generate_ips_from_cidr(&c)?
    } else if let Some(list) = source_ips {
        list.split(',')
            .map(|s| s.trim().parse())
            .collect::<Result<Vec<_>, _>>()?
    } else {
        anyhow::bail!("provide either --cidr or --source-ips");
    };
    let server = webfind::engine::proxy_server::ProxyServer::new(listen_addr, ips);
    server.run().await
}
