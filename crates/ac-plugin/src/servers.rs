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
    /// One row of the builtin table. It yields a [`ServerLit`] rather
    /// than a `Server` because the table is a `const` and a `String`
    /// cannot be built in one; [`From`] turns it into a `Server`.
    #[allow(clippy::new_ret_no_self)]
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
    /// The characters seen on this account the last time it was logged
    /// into, so the screen can offer them before connecting. Names
    /// only: everything else about a character comes from the server.
    #[serde(default)]
    pub characters: Vec<String>,
    /// When this account was last connected with, in seconds since the
    /// epoch. A player with three accounts on a server wants the one
    /// they were just playing, not the one they happened to add first,
    /// so the newest comes back and the rest are offered newest first.
    #[serde(default)]
    pub used: u64,
}

/// The player's own servers and remembered logins, kept in the settings.
#[derive(Clone, Debug, Default)]
pub struct Servers {
    pub custom: Vec<Server>,
    pub logins: Vec<Login>,
    pub last_host: String,
    pub last_account: String,
}

/// Seconds since the epoch, or 0 if the clock is before it.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Settings keys.
const CUSTOM_KEY: &str = "servers.custom";
const LOGINS_KEY: &str = "servers.logins";
const LAST_HOST_KEY: &str = "servers.last_host";
const LAST_ACCOUNT_KEY: &str = "servers.last_account";

impl Servers {
    /// Read the player's servers and logins from the settings.
    pub fn load(settings: &Settings) -> Self {
        let mut s = Servers {
            custom: settings.get(CUSTOM_KEY).unwrap_or_default(),
            logins: settings.get(LOGINS_KEY).unwrap_or_default(),
            last_host: settings.get(LAST_HOST_KEY).unwrap_or_default(),
            last_account: settings.get(LAST_ACCOUNT_KEY).unwrap_or_default(),
        };
        s.carry_over_last_account();
        s
    }

    /// Settings written before accounts were stamped remembered only
    /// one last account, for one server. Give that one a stamp so the
    /// player comes back to where they left off rather than to
    /// whichever account happens to sort first.
    fn carry_over_last_account(&mut self) {
        if self.last_account.is_empty() || self.logins.iter().any(|l| l.used > 0) {
            return;
        }
        let (host, account) = (self.last_host.clone(), self.last_account.clone());
        if let Some(l) = self
            .logins
            .iter_mut()
            .find(|l| l.host == host && l.account.eq_ignore_ascii_case(&account))
        {
            l.used = 1;
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

    /// The remembered accounts for a server host, the one used most
    /// recently first.
    ///
    /// Order matters here: it is what the screen offers, and what is
    /// filled in when the server is chosen. Logins saved before this
    /// was recorded all have the same stamp, so the account name breaks
    /// the tie and the list at least stays put between runs.
    pub fn accounts_for(&self, host: &str) -> Vec<&Login> {
        let mut out: Vec<&Login> = self.logins.iter().filter(|l| l.host == host).collect();
        out.sort_by(|a, b| b.used.cmp(&a.used).then_with(|| a.account.cmp(&b.account)));
        out
    }

    /// The account to fill in for a server: whichever was last used on
    /// *that* server. `None` when none has been saved for it.
    pub fn last_login(&self, host: &str) -> Option<&Login> {
        self.accounts_for(host).into_iter().next()
    }

    /// Remember a login (or update its password/character). An empty
    /// password clears a stored one.
    pub fn remember(&mut self, host: &str, account: &str, password: &str, character: &str) {
        self.last_host = host.to_string();
        self.last_account = account.to_string();
        let now = now_secs();
        if let Some(l) = self
            .logins
            .iter_mut()
            .find(|l| l.host == host && l.account.eq_ignore_ascii_case(account))
        {
            l.password = password.to_string();
            l.character = character.to_string();
            l.used = now;
        } else {
            self.logins.push(Login {
                host: host.to_string(),
                account: account.to_string(),
                password: password.to_string(),
                character: character.to_string(),
                characters: Vec::new(),
                used: now,
            });
        }
    }

    /// Remember the characters an account has, as the server just
    /// listed them.
    ///
    /// This is what lets the screen offer a character before there is a
    /// connection to ask. The list replaces whatever was there: a
    /// character deleted on the server should not linger in a menu. If
    /// the account is not one that was saved, nothing is recorded --
    /// the player did not ask to remember it.
    pub fn note_characters(&mut self, host: &str, account: &str, names: &[String]) {
        if let Some(l) = self
            .logins
            .iter_mut()
            .find(|l| l.host == host && l.account.eq_ignore_ascii_case(account))
        {
            l.characters = names.to_vec();
            // A character that is gone cannot be the one to enter with.
            if !l.character.is_empty() && !names.iter().any(|n| n == &l.character) {
                l.character.clear();
            }
        }
    }

    /// Remember which character an account enters with. Empty means
    /// stop at the character list and choose by hand.
    pub fn set_character(&mut self, host: &str, account: &str, character: &str) {
        if let Some(l) = self
            .logins
            .iter_mut()
            .find(|l| l.host == host && l.account.eq_ignore_ascii_case(account))
        {
            l.character = character.to_string();
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
    #[test]
    fn each_server_remembers_its_own_last_account() {
        // The case this exists for: several accounts on the big server,
        // one on a test shard. Coming back to either brings up the
        // account that was last played *there*.
        let mut s = Servers::default();
        for (host, who) in [
            ("play.coldeve.ac:9000", "main"),
            ("play.coldeve.ac:9000", "mule"),
            ("127.0.0.1:9000", "test"),
            ("play.coldeve.ac:9000", "alt"),
        ] {
            s.remember(host, who, "pw", "");
            bump(&mut s, who);
        }
        assert_eq!(
            s.last_login("play.coldeve.ac:9000").map(|l| &*l.account),
            Some("alt")
        );
        assert_eq!(
            s.last_login("127.0.0.1:9000").map(|l| &*l.account),
            Some("test")
        );
        // Playing the mule again brings the mule back next time.
        s.remember("play.coldeve.ac:9000", "mule", "pw", "");
        bump(&mut s, "mule");
        assert_eq!(
            s.last_login("play.coldeve.ac:9000").map(|l| &*l.account),
            Some("mule")
        );
        // And the test shard is untouched by any of it.
        assert_eq!(
            s.last_login("127.0.0.1:9000").map(|l| &*l.account),
            Some("test")
        );
    }

    /// `remember` stamps with the wall clock, which does not move
    /// between two calls in the same millisecond; nudge the one just
    /// written so the ordering is testable.
    fn bump(s: &mut Servers, account: &str) {
        let top = s.logins.iter().map(|l| l.used).max().unwrap_or(0);
        if let Some(l) = s.logins.iter_mut().find(|l| l.account == account) {
            l.used = top + 1;
        }
    }

    #[test]
    fn accounts_are_offered_newest_first() {
        let mut s = Servers::default();
        for who in ["one", "two", "three"] {
            s.remember("h:9000", who, "", "");
            bump(&mut s, who);
        }
        let order: Vec<&str> = s
            .accounts_for("h:9000")
            .iter()
            .map(|l| &*l.account)
            .collect();
        assert_eq!(order, vec!["three", "two", "one"]);
    }

    #[test]
    fn logins_saved_before_stamping_keep_a_settled_order() {
        // An old settings file has no stamps at all. The list must not
        // shuffle between runs, so the name breaks the tie.
        let mut s = Servers {
            logins: vec![
                Login {
                    host: "h:9000".into(),
                    account: "zoe".into(),
                    ..Default::default()
                },
                Login {
                    host: "h:9000".into(),
                    account: "adam".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let order: Vec<&str> = s
            .accounts_for("h:9000")
            .iter()
            .map(|l| &*l.account)
            .collect();
        assert_eq!(order, vec!["adam", "zoe"]);
        // But the one the old file called the last account wins, so the
        // player comes back where they left off.
        s.last_host = "h:9000".into();
        s.last_account = "zoe".into();
        s.carry_over_last_account();
        assert_eq!(s.last_login("h:9000").map(|l| &*l.account), Some("zoe"));
    }

    #[test]
    fn a_server_with_nothing_saved_offers_nothing() {
        let s = Servers::default();
        assert!(s.accounts_for("play.coldeve.ac:9000").is_empty());
        assert!(s.last_login("play.coldeve.ac:9000").is_none());
    }

    #[test]
    fn forgetting_one_account_leaves_the_others_on_that_server() {
        let mut s = Servers::default();
        s.remember("h:9000", "keep", "pw", "");
        s.remember("h:9000", "drop", "pw", "");
        s.remember("other:9000", "drop", "pw", "");
        s.forget("h:9000", "drop");
        assert_eq!(
            s.accounts_for("h:9000")
                .iter()
                .map(|l| &*l.account)
                .collect::<Vec<_>>(),
            vec!["keep"]
        );
        // The same account name on another server is a different login.
        assert_eq!(s.accounts_for("other:9000").len(), 1);
    }
    #[test]
    fn an_accounts_characters_are_remembered_for_the_menu() {
        // The point: the screen can offer a character before there is
        // any connection to ask.
        let mut s = Servers::default();
        s.remember("h:9000", "main", "pw", "");
        s.note_characters(
            "h:9000",
            "MAIN",
            &["Aldric".to_string(), "Bryn".to_string()],
        );
        let l = s.last_login("h:9000").unwrap();
        assert_eq!(l.characters, ["Aldric", "Bryn"]);
    }

    #[test]
    fn a_character_deleted_on_the_server_leaves_the_menu() {
        let mut s = Servers::default();
        s.remember("h:9000", "main", "pw", "Bryn");
        s.note_characters("h:9000", "main", &["Aldric".to_string()]);
        let l = s.last_login("h:9000").unwrap();
        assert_eq!(l.characters, ["Aldric"]);
        // Bryn was the one to enter with and no longer exists, so the
        // account goes back to stopping at the character list rather
        // than asking for someone who is gone.
        assert_eq!(l.character, "");
    }

    #[test]
    fn a_character_that_survives_stays_the_one_to_enter_with() {
        let mut s = Servers::default();
        s.remember("h:9000", "main", "pw", "Bryn");
        s.note_characters(
            "h:9000",
            "main",
            &["Aldric".to_string(), "Bryn".to_string()],
        );
        assert_eq!(s.last_login("h:9000").unwrap().character, "Bryn");
    }

    #[test]
    fn characters_are_not_recorded_for_an_account_nobody_saved() {
        // Logging in without asking to remember leaves nothing behind.
        let mut s = Servers::default();
        s.note_characters("h:9000", "guest", &["Someone".to_string()]);
        assert!(s.accounts_for("h:9000").is_empty());
    }

    #[test]
    fn choosing_a_character_sticks_to_its_own_account() {
        let mut s = Servers::default();
        s.remember("h:9000", "main", "pw", "");
        s.remember("h:9000", "mule", "pw", "");
        s.set_character("h:9000", "main", "Aldric");
        let by = |who: &str| {
            s.accounts_for("h:9000")
                .into_iter()
                .find(|l| l.account == who)
                .map(|l| l.character.clone())
                .unwrap()
        };
        assert_eq!(by("main"), "Aldric");
        assert_eq!(by("mule"), "", "the other account was not touched");
    }

    #[test]
    fn the_character_menu_survives_a_save_and_load() {
        let mut s = Servers::default();
        s.remember("h:9000", "main", "pw", "");
        s.note_characters("h:9000", "main", &["Aldric".to_string()]);
        s.set_character("h:9000", "main", "Aldric");
        let mut settings = Settings::new();
        s.save(&mut settings);
        let back = Servers::load(&settings);
        let l = back.last_login("h:9000").unwrap();
        assert_eq!(l.characters, ["Aldric"]);
        assert_eq!(l.character, "Aldric");
    }
}
