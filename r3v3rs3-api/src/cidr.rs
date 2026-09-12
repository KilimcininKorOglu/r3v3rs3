use crate::error::Error;
use ipnet::IpNet;
use std::net::IpAddr;

/// Parses a CIDR block or a single IP address. A single address becomes a /32 or /128 network.
pub fn parse_cidr(value: &str) -> Result<IpNet, Error> {
    let value = value.trim();
    if let Ok(net) = value.parse::<IpNet>() {
        return Ok(net.trunc());
    }
    value
        .parse::<IpAddr>()
        .map(IpNet::from)
        .map_err(|_| Error::InvalidCidr {
            cidr: value.to_string(),
        })
}

/// Parses a list of CIDR blocks separated by commas, spaces or new lines.
pub fn parse_cidr_list(value: &str) -> Result<Vec<IpNet>, Error> {
    value
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|item| !item.is_empty())
        .map(parse_cidr)
        .collect()
}

/// Formats a list of CIDR blocks as a comma separated string.
pub fn format_cidr_list(nets: &[IpNet]) -> String {
    nets.iter()
        .map(|net| net.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cidr_and_single_addresses() {
        let nets = parse_cidr_list("10.0.0.0/8, 192.168.1.7\n::1 2001:db8::/32").unwrap();
        assert_eq!(
            format_cidr_list(&nets),
            "10.0.0.0/8, 192.168.1.7/32, ::1/128, 2001:db8::/32"
        );
    }

    #[test]
    fn truncates_host_bits() {
        assert_eq!(parse_cidr("10.1.2.3/8").unwrap().to_string(), "10.0.0.0/8");
    }

    #[test]
    fn rejects_invalid_entries() {
        assert!(matches!(
            parse_cidr_list("10.0.0.0/8, example.com"),
            Err(Error::InvalidCidr { cidr }) if cidr == "example.com"
        ));
        assert!(parse_cidr("10.0.0.0/33").is_err());
    }

    #[test]
    fn empty_list_is_valid() {
        assert!(parse_cidr_list(" , \n").unwrap().is_empty());
    }
}
