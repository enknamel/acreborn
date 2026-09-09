//! Public Asheron's Call server list, and the player's own additions.
//!
//! [`builtin`] is a snapshot of the community list (ACEmulator servers,
//! from `github.com/acresources/serverslist`), Coldeve first as the
//! largest. The player can add their own with [`Servers::add`]; those and
//! the remembered logins live in the settings so they come back next
//! launch.

use serde::{Deserialize, Serialize};

use crate::Settings;

/// One server the client can connect to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    pub name: String,
    pub host: String,
    pub port: u16,
}

impl Server {
    pub const fn new(name: &'static str, host: &'static str, port: u16) -> ServerLit {
        ServerLit { name, host, port }
    }
    /// `host:port`, what the client connects with.
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// A `const`-friendly server literal (the builtin table is `&'static`).
pub struct ServerLit {
    pub name: &'static str,
    pub host: &'static str,
    pub port: u16,
}

impl From<&ServerLit> for Server {
    fn from(s: &ServerLit) -> Self {
        Server {
            name: s.name.into(),
            host: s.host.into(),
            port: s.port,
        }
    }
}

/// The bundled public servers, Coldeve first.
pub fn builtin() -> Vec<Server> {
    BUILTIN.iter().map(Server::from).collect()
}

/// A remembered login for a server: the account, and the password when
/// the player asked to keep it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Login {
    pub host: String,
    pub account: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub character: String,
}

/// The player's own servers and remembered logins, kept in the settings.
#[derive(Clone, Debug, Default)]
pub struct Servers {
    pub custom: Vec<Server>,
    pub logins: Vec<Login>,
    pub last_host: String,
    pub last_account: String,
}

/// Settings keys.
const CUSTOM_KEY: &str = "servers.custom";
const LOGINS_KEY: &str = "servers.logins";
const LAST_HOST_KEY: &str = "servers.last_host";
const LAST_ACCOUNT_KEY: &str = "servers.last_account";

impl Servers {
    /// Read the player's servers and logins from the settings.
    pub fn load(settings: &Settings) -> Self {
        Servers {
            custom: settings.get(CUSTOM_KEY).unwrap_or_default(),
            logins: settings.get(LOGINS_KEY).unwrap_or_default(),
            last_host: settings.get(LAST_HOST_KEY).unwrap_or_default(),
            last_account: settings.get(LAST_ACCOUNT_KEY).unwrap_or_default(),
        }
    }

    /// Write them back.
    pub fn save(&self, settings: &mut Settings) {
        settings.set(CUSTOM_KEY, &self.custom);
        settings.set(LOGINS_KEY, &self.logins);
        settings.set(LAST_HOST_KEY, &self.last_host);
        settings.set(LAST_ACCOUNT_KEY, &self.last_account);
    }

    /// Every server to choose from: the builtin list, then the player's,
    /// with duplicates (same host:port) dropped.
    pub fn all(&self) -> Vec<Server> {
        let mut out = builtin();
        for s in &self.custom {
            if !out.iter().any(|o| o.host == s.host && o.port == s.port) {
                out.push(s.clone());
            }
        }
        out
    }

    /// Add (or update the name of) one of the player's servers.
    pub fn add(&mut self, server: Server) {
        if let Some(existing) = self
            .custom
            .iter_mut()
            .find(|s| s.host == server.host && s.port == server.port)
        {
            existing.name = server.name;
        } else if !builtin()
            .iter()
            .any(|b| b.host == server.host && b.port == server.port)
        {
            self.custom.push(server);
        }
    }

    /// The remembered accounts for a server host.
    pub fn accounts_for(&self, host: &str) -> Vec<&Login> {
        self.logins.iter().filter(|l| l.host == host).collect()
    }

    /// Remember a login (or update its password/character). An empty
    /// password clears a stored one.
    pub fn remember(&mut self, host: &str, account: &str, password: &str, character: &str) {
        self.last_host = host.to_string();
        self.last_account = account.to_string();
        if let Some(l) = self
            .logins
            .iter_mut()
            .find(|l| l.host == host && l.account.eq_ignore_ascii_case(account))
        {
            l.password = password.to_string();
            l.character = character.to_string();
        } else {
            self.logins.push(Login {
                host: host.to_string(),
                account: account.to_string(),
                password: password.to_string(),
                character: character.to_string(),
            });
        }
    }

    /// Forget a remembered login.
    pub fn forget(&mut self, host: &str, account: &str) {
        self.logins
            .retain(|l| !(l.host == host && l.account.eq_ignore_ascii_case(account)));
    }
}

#[rustfmt::skip]
static BUILTIN: &[ServerLit] = &[
// 39 public ACE servers from the community list
    Server::new("Coldeve", "play.coldeve.ac", 9000),
    Server::new("AChard", "a-chard.ddns.net", 9000),
    Server::new("ACPrime", "asheronscall.hopto.org", 9000),
    Server::new("Asheron4Fun.com", "www.asheron4fun.com", 9050),
    Server::new("Buadren AC", "gs1.buadren.com", 9000),
    Server::new("CABA", "z123.zapto.org", 9000),
    Server::new("Conquest", "ACConquest.ddns.net", 9000),
    Server::new("Dekarutide", "dekaru.ac", 9000),
    Server::new("Derptide", "ac.derptide.net", 9000),
    Server::new("Doctide", "doctide.online", 9000),
    Server::new("DragonMoon", "dragonmoonclan.duckdns.org", 9030),
    Server::new("DreamWeave", "play.acdreamweave.com", 9000),
    Server::new("Drunkenfell", "df.drunkenfell.com", 9000),
    Server::new("Ebontide", "ebontide.zapto.org", 9000),
    Server::new("Eversong", "eversong.abrdns.com", 9000),
    Server::new("Frostcull", "frostcull.ddns.net", 9000),
    Server::new("FrostfACE", "172.111.230.127", 9000),
    Server::new("Harvestagain", "harvestagain.ddns.net", 9000),
    Server::new("Infinite Frosthaven", "infinitefrosthaven.hopto.org", 9000),
    Server::new("InfiniteLeaftide", "game.infiniteleaftide.online", 9000),
    Server::new("Jellocull", "ac.jellocull.com", 9000),
    Server::new("LeafDawn", "leafdawn.hopto.org", 9000),
    Server::new("Leafdawning", "leafdawning.duckdns.org", 9000),
    Server::new("Levistras", "levistras.acportalstorm.com", 9000),
    Server::new("Mistwood", "mistwood.ddns.net", 9000),
    Server::new("Modclaim", "ngc1069.dynamic-dns.net", 9000),
    Server::new("Morgentau", "morgentau.online", 9000),
    Server::new("MorningStorm", "morningstorm.redirectme.net", 9070),
    Server::new("Newfoundland", "rftc.duckdns.org", 9000),
    Server::new("Nexus", "135.148.136.179", 9000),
    Server::new("NoESCapeGames", "noescapegames.com", 9000),
    Server::new("PortalStorm", "ac.portalstorm.us.com", 9020),
    Server::new("Shadowgain", "shadowgain.com", 9000),
    Server::new("Shadowland", "shadowland.zapto.org", 9000),
    Server::new("Soulclaim", "soulclaim.ddns.net", 9000),
    Server::new("Sundering", "ace.sunderingac.com", 9000),
    Server::new("The Tower", "the-tower.ddns.net", 9000),
    Server::new("Thistlecrown", "thistlecrown.ddns.net", 9000),
    Server::new("Unfamiliar Shores", "74.50.118.178", 9000),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coldeve_leads_the_builtin_list() {
        let b = builtin();
        assert_eq!(b.first().unwrap().name, "Coldeve");
        assert_eq!(b.first().unwrap().address(), "play.coldeve.ac:9000");
        assert!(b.len() > 30);
    }

    #[test]
    fn custom_servers_and_logins_round_trip_and_merge() {
        let mut s = Servers::default();
        s.add(Server {
            name: "Home".into(),
            host: "127.0.0.1".into(),
            port: 9000,
        });
        // A duplicate of a builtin is not added again.
        s.add(Server {
            name: "Dup".into(),
            host: "play.coldeve.ac".into(),
            port: 9000,
        });
        assert_eq!(s.custom.len(), 1);
        // all() has the builtins plus the one custom.
        assert!(s.all().iter().any(|x| x.address() == "127.0.0.1:9000"));

        s.remember("127.0.0.1:9000", "alice", "pw", "");
        s.remember("127.0.0.1:9000", "bob", "", "");
        assert_eq!(s.accounts_for("127.0.0.1:9000").len(), 2);
        assert_eq!(s.last_account, "bob");

        let mut settings = Settings::new();
        s.save(&mut settings);
        let back = Servers::load(&settings);
        assert_eq!(back.custom, s.custom);
        assert_eq!(back.logins, s.logins);
        assert_eq!(back.last_host, "127.0.0.1:9000");

        s.forget("127.0.0.1:9000", "alice");
        assert_eq!(s.accounts_for("127.0.0.1:9000").len(), 1);
    }
}
