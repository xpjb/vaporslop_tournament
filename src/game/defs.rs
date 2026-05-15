//! Versioned character/item registry. See `plan-versioning.md`.

#[path = "defs_embedded.rs"]
mod defs_embedded;

use crate::game::types::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DefsVersion(pub u32);

#[derive(Clone)]
pub struct Defs {
    current: DefsVersion,
    /// Per-id history, sorted by version descending.
    units: HashMap<String, Vec<(DefsVersion, CharacterDef)>>,
    items: HashMap<String, Vec<(DefsVersion, ItemDef)>>,
}

pub struct DefsTable {
    version: DefsVersion,
    units: HashMap<String, CharacterDef>,
    items: HashMap<String, ItemDef>,
}

/// Resolved snapshot of a [`Build`] pinned to a specific [`DefsTable`].
pub struct Team {
    version: DefsVersion,
    members: Vec<ResolvedMember>,
}

pub struct ResolvedMember {
    pub unit: CharacterDef,
    pub hat: Option<ItemDef>,
    pub left_hand: Option<ItemDef>,
    pub right_hand: Option<ItemDef>,
    pub hand_3: Option<ItemDef>,
    pub hand_4: Option<ItemDef>,
}

impl Defs {
    pub fn load() -> Self {
        let current = DefsVersion(1);
        let mut units = HashMap::<String, Vec<(DefsVersion, CharacterDef)>>::new();
        for c in defs_embedded::build_character_defs() {
            units
                .entry(c.id.clone())
                .or_default()
                .push((current, c));
        }
        let mut items = HashMap::<String, Vec<(DefsVersion, ItemDef)>>::new();
        for i in defs_embedded::build_item_defs() {
            items
                .entry(i.id.clone())
                .or_default()
                .push((current, i));
        }
        for v in units.values_mut() {
            v.sort_by(|a, b| b.0.cmp(&a.0));
        }
        for v in items.values_mut() {
            v.sort_by(|a, b| b.0.cmp(&a.0));
        }
        Self {
            current,
            units,
            items,
        }
    }

    pub fn current_version(&self) -> DefsVersion {
        self.current
    }

    pub fn current_table(&self) -> DefsTable {
        self.table_at(self.current)
            .expect("current version always resolves a full DefsTable")
    }

    /// [`None`] when `version` is out of range (`0` or above the loaded `current`).
    pub fn table_at(&self, version: DefsVersion) -> Option<DefsTable> {
        if version.0 == 0 || version.0 > self.current.0 {
            return None;
        }
        let mut units = HashMap::new();
        for (id, hist) in &self.units {
            if let Some((_, d)) = hist.iter().find(|(v, _)| v.0 <= version.0) {
                units.insert(id.clone(), d.clone());
            }
        }
        let mut items = HashMap::new();
        for (id, hist) in &self.items {
            if let Some((_, d)) = hist.iter().find(|(v, _)| v.0 <= version.0) {
                items.insert(id.clone(), d.clone());
            }
        }
        Some(DefsTable {
            version,
            units,
            items,
        })
    }

    #[cfg(test)]
    pub(crate) fn inject_test(
        current: DefsVersion,
        units: HashMap<String, Vec<(DefsVersion, CharacterDef)>>,
        items: HashMap<String, Vec<(DefsVersion, ItemDef)>>,
    ) -> Self {
        let mut s = Self {
            current,
            units,
            items,
        };
        for v in s.units.values_mut() {
            v.sort_by(|a, b| b.0.cmp(&a.0));
        }
        for v in s.items.values_mut() {
            v.sort_by(|a, b| b.0.cmp(&a.0));
        }
        s
    }

    /// Test-only: mutate `current` without adding rows (exercises out-of-range table_at).
    #[cfg(test)]
    pub(crate) fn set_current_for_test(&mut self, current: DefsVersion) {
        self.current = current;
    }
}

impl DefsTable {
    pub fn version(&self) -> DefsVersion {
        self.version
    }

    pub fn unit(&self, id: &str) -> Option<&CharacterDef> {
        self.units.get(id)
    }

    pub fn item(&self, id: &str) -> Option<&ItemDef> {
        self.items.get(id)
    }

    pub fn sorted_characters(&self) -> Vec<CharacterDef> {
        let mut v: Vec<CharacterDef> = self.units.values().cloned().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    pub fn sorted_items(&self) -> Vec<ItemDef> {
        let mut v: Vec<ItemDef> = self.items.values().cloned().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    pub fn resolve(&self, build: &Build) -> Option<Team> {
        let mut members = Vec::with_capacity(build.team.len());
        for m in &build.team {
            let unit = self.unit(&m.def_id)?.clone();
            let hat = match &m.hat {
                None => None,
                Some(id) => Some(self.item(id)?.clone()),
            };
            let left_hand = match &m.left_hand {
                None => None,
                Some(id) => Some(self.item(id)?.clone()),
            };
            let right_hand = match &m.right_hand {
                None => None,
                Some(id) => Some(self.item(id)?.clone()),
            };
            let hand_3 = match &m.hand_3 {
                None => None,
                Some(id) => Some(self.item(id)?.clone()),
            };
            let hand_4 = match &m.hand_4 {
                None => None,
                Some(id) => Some(self.item(id)?.clone()),
            };
            members.push(ResolvedMember {
                unit,
                hat,
                left_hand,
                right_hand,
                hand_3,
                hand_4,
            });
        }
        Some(Team {
            version: self.version,
            members,
        })
    }

    #[cfg(test)]
    pub(crate) fn from_maps(
        version: DefsVersion,
        units: HashMap<String, CharacterDef>,
        items: HashMap<String, ItemDef>,
    ) -> Self {
        Self {
            version,
            units,
            items,
        }
    }

    #[cfg(test)]
    pub(crate) fn clone_maps(&self) -> (HashMap<String, CharacterDef>, HashMap<String, ItemDef>) {
        (self.units.clone(), self.items.clone())
    }
}

impl Team {
    pub fn version(&self) -> DefsVersion {
        self.version
    }

    pub(crate) fn members(&self) -> &[ResolvedMember] {
        &self.members
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::types::TeamMember;

    #[test]
    fn defs_table_at_picks_latest_le_version() {
        let mut units = HashMap::new();
        let v1_def = CharacterDef {
            id: "x".into(),
            name: "X".into(),
            sprite: "x.webp".into(),
            cost: 1,
            might: 1,
            reflexes: 1,
            wisdom: 1,
            hp: 100,
            properties: vec![],
        };
        let v3_def = CharacterDef {
            hp: 200,
            ..v1_def.clone()
        };
        units.insert(
            "x".into(),
            vec![
                (DefsVersion(3), v3_def.clone()),
                (DefsVersion(1), v1_def.clone()),
            ],
        );

        let defs = Defs::inject_test(
            DefsVersion(5),
            units,
            HashMap::new(),
        );
        let t = defs.table_at(DefsVersion(2)).expect("table");
        assert_eq!(t.unit("x").expect("exists").hp, v1_def.hp);
        let t3 = defs.table_at(DefsVersion(3)).expect("table v3");
        assert_eq!(t3.unit("x").expect("exists").hp, v3_def.hp);
    }

    #[test]
    fn defs_table_at_returns_none_for_unknown_version() {
        let defs = Defs::load();
        assert!(defs.table_at(DefsVersion(99)).is_none());
        assert!(defs.table_at(DefsVersion(0)).is_none());
    }

    #[test]
    fn resolve_returns_none_on_unknown_id() {
        let defs = Defs::load().current_table();
        let build = Build {
            team: vec![TeamMember {
                def_id: "not_real_def_id_xx".into(),
                hat: None,
                left_hand: None,
                right_hand: None,
                hand_3: None,
                hand_4: None,
            }],
        };
        assert!(defs.resolve(&build).is_none());
    }

    #[test]
    fn replay_against_old_version_matches_recorded_outcome() {
        use crate::game::combat::resolve_v1;
        use crate::game::rng::Rng;

        let live = Defs::load().current_table();
        let meme_v1_row = live.unit("meme_man").expect("fixture").clone();
        let meme_v2_row = CharacterDef {
            hp: 200,
            might: meme_v1_row.might + 20,
            ..meme_v1_row.clone()
        };
        let pic = live.unit("picardia").expect("fixture").clone();

        let mut hist_units = HashMap::new();
        hist_units.insert(
            "meme_man".into(),
            vec![
                (DefsVersion(2), meme_v2_row),
                (
                    DefsVersion(1),
                    CharacterDef {
                        hp: 18,
                        ..meme_v1_row.clone()
                    },
                ),
            ],
        );
        hist_units.insert("picardia".into(), vec![(DefsVersion(1), pic)]);

        let mut registry = Defs::inject_test(DefsVersion(2), hist_units, HashMap::new());

        let left = Build {
            team: vec![TeamMember {
                def_id: "meme_man".into(),
                hat: None,
                left_hand: None,
                right_hand: None,
                hand_3: None,
                hand_4: None,
            }],
        };
        let right = Build {
            team: vec![TeamMember {
                def_id: "picardia".into(),
                hat: None,
                left_hand: None,
                right_hand: None,
                hand_3: None,
                hand_4: None,
            }],
        };

        let table_v1 = registry.table_at(DefsVersion(1)).expect("snapshot");
        let team_l = table_v1.resolve(&left).expect("resolve");
        let team_r = table_v1.resolve(&right).expect("resolve");
        let seed = 0xC0FFEE;
        let mut rng = Rng::new(seed);
        let res_v1 = resolve_v1(&table_v1, &team_l, &team_r, &mut rng);

        registry.set_current_for_test(DefsVersion(2));
        let table_v2 = registry.current_table();
        let team_lv2 = table_v2.resolve(&left).expect("resolve");
        let team_rv2 = table_v2.resolve(&right).expect("resolve");
        let mut rng2 = Rng::new(seed);
        let res_v2 = resolve_v1(&table_v2, &team_lv2, &team_rv2, &mut rng2);

        assert_ne!(
            res_v1.events.len(),
            res_v2.events.len(),
            "different meme_man stats across versions should change the scripted fight length"
        );

        let mut rng_replay = Rng::new(seed);
        let replay = resolve_v1(&table_v1, &team_l, &team_r, &mut rng_replay);
        assert_eq!(replay.winner, res_v1.winner);
        assert_eq!(replay.events.len(), res_v1.events.len());
    }
}
