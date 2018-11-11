use convert::TryFrom;
use futures::prelude::*;
use std::{fmt, net};
use std::time::Instant;
use tokio::timer::Delay;
use trust_dns_resolver::{
    config::{ResolverConfig, ResolverOpts},
    lookup_ip::{LookupIp},
    system_conf,
    AsyncResolver,
    BackgroundLookupIp,
};

pub use trust_dns_resolver::error::{ResolveError, ResolveErrorKind};

use app::config::Config;
use transport::tls;

#[derive(Clone)]
pub struct Resolver {
    resolver: AsyncResolver,
}

#[derive(Debug)]
pub enum Error {
    NoAddressesFound,
    ResolutionFailed(ResolveError),
}

pub enum Response {
    Exists(LookupIp),
    DoesNotExist { retry_after: Option<Instant> },
}

pub struct IpAddrFuture(::logging::ContextualFuture<Ctx, BackgroundLookupIp>);

pub struct RefineFuture(::logging::ContextualFuture<Ctx, BackgroundLookupIp>);

pub type IpAddrListFuture = Box<Future<Item = Response, Error = ResolveError> + Send>;

/// A valid DNS name.
///
/// This is an alias of the strictly-validated `tls::DnsName` based on the
/// premise that we only need to support DNS names for which one could get a
/// valid certificate.
pub type Name = tls::DnsName;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Suffix {
    Root, // The `.` suffix.
    Name(Name),
}

struct Ctx(Name);

pub struct Refine {
    pub name: Name,
    pub valid_until: Instant,
}

impl fmt::Display for Ctx {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "dns={}", self.0)
    }
}

impl fmt::Display for Suffix {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Suffix::Root => write!(f, "."),
            Suffix::Name(n) => n.fmt(f),
        }
    }
}

impl From<Name> for Suffix {
    fn from(n: Name) -> Self {
        Suffix::Name(n)
    }
}

impl Resolver {

    /// Construct a new `Resolver` from environment variables and system
    /// configuration.
    ///
    /// # Returns
    ///
    /// Either a tuple containing a new `Resolver` and the background task to
    /// drive that resolver's futures, or an error if the system configuration
    /// could not be parsed.
    ///
    /// TODO: This should be infallible like it is in the `domain` crate.
    pub fn from_system_config_and_env(env_config: &Config)
        -> Result<(Self, impl Future<Item = (), Error = ()> + Send), ResolveError> {
        let (config, opts) = system_conf::read_system_conf()?;
        let opts = env_config.configure_resolver_opts(opts);
        trace!("DNS config: {:?}", &config);
        trace!("DNS opts: {:?}", &opts);
        Ok(Self::new(config, opts))
    }


    /// NOTE: It would be nice to be able to return a named type rather than
    ///       `impl Future` for the background future; it would be called
    ///       `Background` or `ResolverBackground` if that were possible.
    pub fn new(config: ResolverConfig,  mut opts: ResolverOpts)
        -> (Self, impl Future<Item = (), Error = ()> + Send)
    {
        // Disable Trust-DNS's caching.
        opts.cache_size = 0;
        let (resolver, background) = AsyncResolver::new(config, opts);
        let resolver = Resolver {
            resolver,
        };
        (resolver, background)
    }

    pub fn resolve_all_ips(&self, deadline: Instant, name: &Name) -> IpAddrListFuture {
        let lookup = self.resolver.lookup_ip(name.as_ref());

        // FIXME this delay logic is really confusing...
        let f = Delay::new(deadline)
            .then(move |_| {
                trace!("after delay");
                lookup
            })
            .then(move |result| {
                trace!("completed with {:?}", &result);
                result.map(Response::Exists).or_else(|e| {
                    if let &ResolveErrorKind::NoRecordsFound { valid_until, .. } = e.kind() {
                        Ok(Response::DoesNotExist { retry_after: valid_until })
                    } else {
                        Err(e)
                    }
                })
            });

        Box::new(::logging::context_future(Ctx(name.clone()), f))
    }

    pub fn resolve_one_ip(&self, name: &Name) -> IpAddrFuture {
        let f = self.resolver.lookup_ip(name.as_ref());
        IpAddrFuture(::logging::context_future(Ctx(name.clone()), f))
    }

    pub fn refine(&self, name: &Name) -> RefineFuture {
        let f = self.resolver.lookup_ip(name.as_ref());
        RefineFuture(::logging::context_future(Ctx(name.clone()), f))
    }
}

/// Note: `AsyncResolver` does not implement `Debug`, so we must manually
///       implement this.
impl fmt::Debug for Resolver {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Resolver")
            .field("resolver", &"...")
            .finish()
    }
}

impl Future for IpAddrFuture {
    type Item = net::IpAddr;
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let ips = try_ready!(self.0.poll().map_err(Error::ResolutionFailed));
        ips.iter()
            .next()
            .map(Async::Ready)
            .ok_or_else(|| Error::NoAddressesFound)
    }
}

impl Future for RefineFuture {
    type Item = Refine;
    type Error = ResolveError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let lookup = try_ready!(self.0.poll());
        let valid_until = lookup.valid_until();

        let n = lookup.query().name();
        let name = Name::try_from(n.to_ascii().as_bytes())
            .expect("Name returned from resolver must be valid");

        let refine = Refine { name, valid_until };
        Ok(Async::Ready(refine))
    }
}

#[cfg(test)]
mod tests {
    use super::Name;

    #[test]
    fn test_dns_name_parsing() {
        // Stack sure `dns::Name`'s validation isn't too strict. It is
        // implemented in terms of `webpki::DNSName` which has many more tests
        // at https://github.com/briansmith/webpki/blob/master/tests/dns_name_tests.rs.
        use convert::TryFrom;

        struct Case {
            input: &'static str,
            output: &'static str,
        }

        static VALID: &[Case] = &[
            // Almost all digits and dots, similar to IPv4 addresses.
            Case { input: "1.2.3.x", output: "1.2.3.x", },
            Case { input: "1.2.3.x", output: "1.2.3.x", },
            Case { input: "1.2.3.4A", output: "1.2.3.4a", },
            Case { input: "a.b.c.d", output: "a.b.c.d", },

            // Uppercase letters in labels
            Case { input: "A.b.c.d", output: "a.b.c.d", },
            Case { input: "a.mIddle.c", output: "a.middle.c", },
            Case { input: "a.b.c.D", output: "a.b.c.d", },

            // Absolute
            Case { input: "a.b.c.d.", output: "a.b.c.d.", },
        ];

        for case in VALID {
            let name = Name::try_from(case.input.as_bytes());
            assert_eq!(name.as_ref().map(|x| x.as_ref()), Ok(case.output));
        }

        static INVALID: &[&str] = &[
            // These are not in the "preferred name syntax" as defined by
            // https://tools.ietf.org/html/rfc1123#section-2.1. In particular
            // the last label only has digits.
            "1.2.3.4",
            "a.1.2.3",
            "1.2.x.3",
        ];

        for case in INVALID {
            assert!(Name::try_from(case.as_bytes()).is_err());
        }
    }
}
