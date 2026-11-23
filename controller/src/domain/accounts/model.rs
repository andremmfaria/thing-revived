//! Account snapshot: `/etc/passwd`, `/etc/group` and a reduced `/etc/shadow`
//! parsed from strings. Hashes are dropped while parsing and never stored.
//! Time arrives as days since the epoch (the unit shadow uses).

use super::names::KIOSK_USER;

/// Managed users and groups have an id in this range (plan section 6).
pub const MANAGED_ID_MIN: u32 = 1000;
pub const MANAGED_ID_MAX: u32 = 59999;
/// The one authority group (decision D1).
pub const ADMIN_GROUP: &str = "sudo";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: String,
    pub shell: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub gid: u32,
    pub members: Vec<String>,
}

/// One shadow row reduced to what the menu may know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowInfo {
    pub name: String,
    /// Hash starts with `!` (usermod --lock or an account that never got one).
    pub locked: bool,
    /// A hash exists behind any lock marker: not empty, not `*`, not bare `!`.
    pub has_password: bool,
    /// Account expiry in days since the epoch, if set.
    pub expire_days: Option<i64>,
}

impl ShadowInfo {
    fn expired(&self, today_days: i64) -> bool {
        matches!(self.expire_days, Some(e) if e <= today_days)
    }
}

/// A row of `user_list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserView {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: String,
    pub shell: String,
    pub locked: bool,
    pub admin: bool,
    pub system: bool,
    pub protected: bool,
    pub groups: Vec<String>,
}

pub fn is_system_id(id: u32) -> bool {
    id < MANAGED_ID_MIN
}

pub fn is_managed_id(id: u32) -> bool {
    (MANAGED_ID_MIN..=MANAGED_ID_MAX).contains(&id)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub users: Vec<User>,
    pub groups: Vec<Group>,
    pub shadow: Vec<ShadowInfo>,
}

fn data_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
}

/// Malformed rows are skipped, not fatal: one bad line must not hide the rest.
pub fn parse_passwd(text: &str) -> Vec<User> {
    data_lines(text)
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() < 7 {
                return None;
            }
            Some(User {
                name: f[0].to_string(),
                uid: f[2].parse().ok()?,
                gid: f[3].parse().ok()?,
                home: f[5].to_string(),
                shell: f[6].to_string(),
            })
        })
        .collect()
}

pub fn parse_group(text: &str) -> Vec<Group> {
    data_lines(text)
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() < 4 {
                return None;
            }
            Some(Group {
                name: f[0].to_string(),
                gid: f[2].parse().ok()?,
                members: f[3].split(',').filter(|m| !m.is_empty()).map(str::to_string).collect(),
            })
        })
        .collect()
}

/// Reduces each row at the boundary; the hash text is dropped here.
pub fn parse_shadow(text: &str) -> Vec<ShadowInfo> {
    data_lines(text)
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() < 2 || f[0].is_empty() {
                return None;
            }
            let hash = f[1];
            let rest = hash.strip_prefix('!').unwrap_or(hash);
            Some(ShadowInfo {
                name: f[0].to_string(),
                locked: hash.starts_with('!'),
                has_password: !rest.is_empty() && !rest.starts_with('*') && !rest.starts_with('!'),
                expire_days: f.get(7).and_then(|e| e.parse().ok()),
            })
        })
        .collect()
}

impl Snapshot {
    pub fn parse(passwd: &str, group: &str, shadow: &str) -> Snapshot {
        Snapshot { users: parse_passwd(passwd), groups: parse_group(group), shadow: parse_shadow(shadow) }
    }

    pub fn user(&self, name: &str) -> Option<&User> {
        self.users.iter().find(|u| u.name == name)
    }

    pub fn group(&self, name: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.name == name)
    }

    pub fn shadow_of(&self, name: &str) -> Option<&ShadowInfo> {
        self.shadow.iter().find(|s| s.name == name)
    }

    /// Every user name and group name, for the exists check.
    pub fn taken_names(&self) -> impl Iterator<Item = &str> {
        self.users.iter().map(|u| u.name.as_str()).chain(self.groups.iter().map(|g| g.name.as_str()))
    }

    /// Names of every group the user belongs to: the primary group by gid
    /// plus listed memberships, sorted, no duplicates.
    pub fn groups_of(&self, user: &str) -> Vec<String> {
        let primary = self.user(user).map(|u| u.gid);
        let mut out: Vec<String> = self
            .groups
            .iter()
            .filter(|g| Some(g.gid) == primary || g.members.iter().any(|m| m == user))
            .map(|g| g.name.clone())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    pub fn is_in_group(&self, user: &str, group: &str) -> bool {
        self.groups_of(user).iter().any(|g| g == group)
    }

    /// Locked here means the menu must show it as locked: lock marker or an
    /// expiry in the past (the lock command sets expiry 1).
    pub fn is_locked(&self, name: &str, today_days: i64) -> bool {
        match self.shadow_of(name) {
            Some(s) => s.locked || s.expired(today_days),
            None => true,
        }
    }

    pub fn has_password(&self, name: &str) -> bool {
        self.shadow_of(name).is_some_and(|s| s.has_password)
    }

    /// Admin: member of `sudo`, not locked, not expired, usable hash.
    pub fn is_effective_admin(&self, name: &str, today_days: i64) -> bool {
        self.user(name).is_some()
            && self.is_in_group(name, ADMIN_GROUP)
            && !self.is_locked(name, today_days)
            && self.has_password(name)
    }

    pub fn effective_admins(&self, today_days: i64) -> Vec<&str> {
        self.users
            .iter()
            .filter(|u| self.is_effective_admin(&u.name, today_days))
            .map(|u| u.name.as_str())
            .collect()
    }

    pub fn managed_user_count(&self) -> usize {
        self.users.iter().filter(|u| is_managed_id(u.uid)).count()
    }

    /// Groups the owner created: managed gid and not a user's private group.
    pub fn managed_group_count(&self) -> usize {
        self.groups.iter().filter(|g| is_managed_id(g.gid) && self.user(&g.name).is_none()).count()
    }

    pub fn view(&self, name: &str, today_days: i64) -> Option<UserView> {
        let u = self.user(name)?;
        Some(UserView {
            name: u.name.clone(),
            uid: u.uid,
            gid: u.gid,
            home: u.home.clone(),
            shell: u.shell.clone(),
            locked: self.is_locked(name, today_days),
            admin: self.is_effective_admin(name, today_days),
            system: is_system_id(u.uid),
            protected: u.name == KIOSK_USER,
            groups: self.groups_of(name),
        })
    }

    pub fn views(&self, today_days: i64) -> Vec<UserView> {
        self.users.iter().filter_map(|u| self.view(&u.name, today_days)).collect()
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub const TODAY: i64 = 20_000;

    pub fn snap() -> Snapshot {
        Snapshot::parse(
            "root:x:0:0:root:/root:/bin/sh\n\
             nobody:x:65534:65534:nobody:/:/bin/false\n\
             kiosk:x:1000:1000::/var/home/kiosk:/usr/bin/bash\n\
             alice:x:1001:1001::/var/home/alice:/usr/bin/bash\n\
             bob:x:1002:1002::/var/home/bob:/usr/bin/bash\n",
            "root:x:0:\nadm:x:4:kiosk\nsudo:x:27:kiosk,alice\nkiosk:x:1000:\nalice:x:1001:\nbob:x:1002:\nstaff2:x:1003:bob\n",
            "root:*:19000:0:99999:7:::\n\
             nobody:*:19000:0:99999:7:::\n\
             kiosk:$6$salt$hash:19000:0:99999:7:::\n\
             alice:$6$salt$hash:19000:0:99999:7:::\n\
             bob:!$6$salt$hash:19000:0:99999:7:::\n",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    #[test]
    fn parses_rows_and_skips_garbage() {
        let users = parse_passwd("# c\n\nbad line\nx:x:notnum:1::/h:/s\na:x:1:2:g:/h:/s\n");
        assert_eq!(users.len(), 1);
        assert_eq!(users[0], User { name: "a".into(), uid: 1, gid: 2, home: "/h".into(), shell: "/s".into() });
    }

    #[test]
    fn shadow_reduction_drops_hash() {
        let s = parse_shadow("a:$6$x:1::::::\nb:!:1\nc:!$6$x:1\nd:*:1\ne::1\nf:!!:1\ng:x:1:0:0:0:::20\nh:x:1:0:0:0::5:\n");
        let by = |n: &str| s.iter().find(|x| x.name == n).unwrap().clone();
        assert_eq!((by("a").locked, by("a").has_password), (false, true));
        assert_eq!((by("b").locked, by("b").has_password), (true, false));
        assert_eq!((by("c").locked, by("c").has_password), (true, true));
        assert_eq!((by("d").locked, by("d").has_password), (false, false));
        assert_eq!((by("e").locked, by("e").has_password), (false, false));
        assert_eq!((by("f").locked, by("f").has_password), (true, false));
        assert_eq!(by("h").expire_days, Some(5));
        assert!(format!("{s:?}").find("$6$").is_none(), "hash must not survive parsing");
    }

    #[test]
    fn groups_include_primary_and_members() {
        let s = snap();
        assert_eq!(s.groups_of("kiosk"), vec!["adm", "kiosk", "sudo"]);
        assert_eq!(s.groups_of("bob"), vec!["bob", "staff2"]);
    }

    #[test]
    fn effective_admin_rules() {
        let s = snap();
        assert_eq!(s.effective_admins(TODAY), vec!["kiosk", "alice"]);
        // bob is locked and not in sudo
        assert!(!s.is_effective_admin("bob", TODAY));
        let mut expired = s.clone();
        expired.shadow.iter_mut().find(|x| x.name == "alice").unwrap().expire_days = Some(1);
        assert_eq!(expired.effective_admins(TODAY), vec!["kiosk"]);
        let mut nopw = s.clone();
        nopw.shadow.iter_mut().find(|x| x.name == "kiosk").unwrap().has_password = false;
        assert_eq!(nopw.effective_admins(TODAY), vec!["alice"]);
        let mut locked = s;
        locked.shadow.iter_mut().find(|x| x.name == "kiosk").unwrap().locked = true;
        assert_eq!(locked.effective_admins(TODAY), vec!["alice"]);
    }

    #[test]
    fn missing_shadow_row_counts_as_locked() {
        let mut s = snap();
        s.shadow.retain(|x| x.name != "alice");
        assert!(s.is_locked("alice", TODAY));
        assert!(!s.is_effective_admin("alice", TODAY));
    }

    #[test]
    fn counts_and_views() {
        let s = snap();
        assert_eq!(s.managed_user_count(), 3);
        assert_eq!(s.managed_group_count(), 1, "only staff2; private groups do not count");
        let v = s.view("kiosk", TODAY).unwrap();
        assert!(v.protected && v.admin && !v.system && !v.locked);
        let r = s.view("root", TODAY).unwrap();
        assert!(r.system && !r.admin);
        assert_eq!(s.views(TODAY).len(), 5);
        assert!(s.view("ghost", TODAY).is_none());
        assert_eq!(s.taken_names().count(), 5 + 7);
    }
}
