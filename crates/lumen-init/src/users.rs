//! Minimal /etc/passwd and /etc/group lookup.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: String,
    pub shell: String,
}

pub fn parse_passwd(text: &str, name: &str) -> Option<User> {
    text.lines().find_map(|line| {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() >= 7 && f[0] == name {
            Some(User {
                name: f[0].to_string(),
                uid: f[2].parse().ok()?,
                gid: f[3].parse().ok()?,
                home: f[5].to_string(),
                shell: f[6].to_string(),
            })
        } else {
            None
        }
    })
}

/// Supplementary groups of `user` (gids of groups listing it as a member).
pub fn parse_groups(text: &str, user: &str) -> Vec<u32> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split(':').collect();
            if f.len() >= 4 && f[3].split(',').any(|m| m.trim() == user) {
                f[2].parse().ok()
            } else {
                None
            }
        })
        .collect()
}

pub fn group_gid(text: &str, group: &str) -> Option<u32> {
    text.lines().find_map(|line| {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() >= 3 && f[0] == group { f[2].parse().ok() } else { None }
    })
}

pub fn lookup(name: &str) -> Option<(User, Vec<u32>)> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    let user = parse_passwd(&passwd, name)?;
    let groups = std::fs::read_to_string("/etc/group")
        .map(|g| parse_groups(&g, name))
        .unwrap_or_default();
    Some((user, groups))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "root:x:0:0:root:/root:/bin/lsh\nlumen:x:1000:1000:Lumen:/home/lumen:/bin/lsh\n";
    const GROUP: &str = "root:x:0:root\nvideo:x:27:root,lumen\ninput:x:23:lumen\nseat:x:900: lumen \nwheel:x:10:root\n";

    #[test]
    fn finds_user_and_groups() {
        let u = parse_passwd(PASSWD, "lumen").unwrap();
        assert_eq!((u.uid, u.gid), (1000, 1000));
        assert_eq!(u.home, "/home/lumen");
        assert_eq!(parse_groups(GROUP, "lumen"), vec![27, 23, 900]);
        assert_eq!(group_gid(GROUP, "seat"), Some(900));
        assert!(parse_passwd(PASSWD, "nobody").is_none());
    }
}
