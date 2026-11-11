//! An area as the binary lists it: its name, the frame kinds it owns and
//! the commands it has, known before anything is built, so the list can be
//! checked and its kinds tabled (`parvi kinds`) without starting a node.
//! The node holds a built area to its listing.

use parvi_core::Error;
use parvi_core::frame::{self, Kinds, RESERVED};

use crate::{Build, own};

#[derive(Clone, Copy)]
pub struct Listing {
    /// Its config section, its name in the hello's `off`, and in errors.
    pub name: &'static str,
    /// The frame kinds it owns, both ways: a client's frame of one goes to
    /// its channel.
    pub kinds: Kinds,
    /// Its commands' names (`noun_verb`); the node refuses an area whose
    /// built commands differ.
    pub commands: &'static [&'static str],
    pub build: Build,
}

/// Refuses areas that share a name, a frame kind or a command, naming
/// both, any that takes a reserved kind or one of the node's own commands
/// ([`crate::own::COMMANDS`]), and commands not named `noun_verb`.
pub fn check(areas: &[Listing]) -> Result<(), Error> {
    let conflict = |message: String| Error::new("area_conflict", message);
    for (i, area) in areas.iter().enumerate() {
        if let Some(owner) = frame::reserved(area.kinds) {
            return Err(conflict(format!(
                "area {} owns frame kinds of {owner}",
                area.name
            )));
        }
        if let Some(command) = area.commands.iter().find(|c| !well_named(c)) {
            return Err(conflict(format!(
                "area {}: command {command} is not named noun_verb",
                area.name
            )));
        }
        if let Some(command) = area.commands.iter().find(|c| own::COMMANDS.contains(c)) {
            return Err(conflict(format!(
                "area {} has the command {command}, which is the node's own",
                area.name
            )));
        }
        for other in &areas[..i] {
            if other.name == area.name {
                return Err(conflict(format!("two areas are named {}", area.name)));
            }
            if frame::overlap(other.kinds, area.kinds) {
                return Err(conflict(format!(
                    "areas {} and {} own the same frame kinds",
                    other.name, area.name
                )));
            }
            if let Some(command) = area.commands.iter().find(|c| other.commands.contains(c)) {
                return Err(conflict(format!(
                    "areas {} and {} both have the command {command}",
                    other.name, area.name
                )));
            }
        }
    }
    Ok(())
}

/// Whether a command's name reads as `noun_verb`: a noun, then a verb of
/// one or more words (`formation_set_type` is `parvi formation set-type`),
/// lowercase letters and digits between single underscores.
pub fn well_named(name: &str) -> bool {
    let word = |w: &str| {
        !w.is_empty()
            && w.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    };
    name.split('_').count() >= 2 && name.split('_').all(word)
}

/// Who owns which frame kinds: the reserved ones and each area's, in the
/// order of their first kind.
pub fn kinds_table(areas: &[Listing]) -> String {
    let mut rows: Vec<(u8, String, &str)> = RESERVED
        .iter()
        .map(|(owner, kinds)| (*kinds, *owner))
        .chain(areas.iter().map(|a| (a.kinds, a.name)))
        .filter_map(|(kinds, owner)| {
            let first = kinds.iter().map(|r| *r.start()).min()?;
            Some((first, frame::describe(kinds), owner))
        })
        .collect();
    rows.sort();
    let width = rows.iter().map(|(_, k, _)| k.len()).max().unwrap_or(0);
    rows.iter()
        .map(|(_, kinds, owner)| format!("{kinds:width$}  {owner}\n"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Built, Setup};

    fn none(_: &Setup) -> anyhow::Result<Built> {
        anyhow::bail!("not built here")
    }

    fn listing(name: &'static str, kinds: Kinds, commands: &'static [&'static str]) -> Listing {
        Listing {
            name,
            kinds,
            commands,
            build: none,
        }
    }

    #[test]
    fn listings_that_clash_are_refused_naming_both() {
        let a = listing("a", &[0x40..=0x41], &["a_list"]);
        let b = listing("b", &[0x42..=0x42], &["b_list"]);
        assert!(check(&[a, b]).is_ok());
        let message = |areas: &[Listing]| check(areas).unwrap_err().message;
        assert_eq!(message(&[a, a]), "two areas are named a");
        let kinds = listing("c", &[0x41..=0x41], &[]);
        assert_eq!(
            message(&[a, kinds]),
            "areas a and c own the same frame kinds"
        );
        let command = listing("c", &[], &["a_list"]);
        assert_eq!(
            message(&[a, command]),
            "areas a and c both have the command a_list"
        );
        let hello = listing("c", &[0x00..=0x00], &[]);
        assert_eq!(
            message(&[hello]),
            "area c owns frame kinds of the node's hello"
        );
        let own = listing("c", &[], &["config_show"]);
        assert_eq!(
            message(&[own]),
            "area c has the command config_show, which is the node's own"
        );
        let named = listing("c", &[], &["list"]);
        assert_eq!(
            message(&[named]),
            "area c: command list is not named noun_verb"
        );
        for bad in [
            "thing__list",
            "_list",
            "thing_",
            "thing_set-type",
            "Thing_list",
        ] {
            assert!(!well_named(bad), "{bad}");
        }
        assert!(well_named("thing_list"));
        assert!(well_named("formation_set_type"));
    }

    #[test]
    fn the_kinds_table_lists_reserved_and_listed_kinds_in_order() {
        let a = listing("lights", &[0x40..=0x41], &[]);
        let quiet = listing("quiet", &[], &[]);
        let table = kinds_table(&[a, quiet]);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines[0], "0x00        the node's hello");
        assert_eq!(lines[1], "0x03, 0x1b  the fleet's peer list");
        assert_eq!(lines[2], "0x40-0x41   lights");
        assert_eq!(lines[3], "0xb0-0xbf   view control");
        assert!(!table.contains("quiet"), "an area with no kinds has no row");
    }
}
