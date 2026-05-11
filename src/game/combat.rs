use crate::game::data::*;
use crate::game::rng::Rng;
use crate::game::types::*;
use serde::{Deserialize, Serialize};

/// A combatant materialized for battle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Combatant {
    pub uid: u32, // unique id within battle
    pub def_id: String,
    pub sprite: String,
    pub hat_id: Option<String>,
    pub left_hand_id: Option<String>,
    pub right_hand_id: Option<String>,
    #[serde(default)]
    pub hand_3_id: Option<String>,
    #[serde(default)]
    pub hand_4_id: Option<String>,
    pub hat_sprite: Option<String>,
    pub left_hand_sprite: Option<String>,
    pub right_hand_sprite: Option<String>,
    #[serde(default)]
    pub hand_3_sprite: Option<String>,
    #[serde(default)]
    pub hand_4_sprite: Option<String>,
    /// Intrinsic max HP (def + gear); aura adds `formation_hp_bonus`.
    pub max_hp: i32,
    pub hp: i32,
    pub might: i32,
    pub reflexes: i32,
    pub wisdom: i32,
    pub properties: Vec<Property>,
    pub frozen_turns: i32,
    pub side: u8, // 0 or 1
    #[serde(default)]
    applied_front_might: i32,
    #[serde(default)]
    applied_front_reflexes: i32,
    #[serde(default)]
    applied_front_wisdom: i32,
    #[serde(default)]
    pub formation_hp_bonus: i32,
    #[serde(default)]
    pub applied_enemy_reflex_debuff: i32,
    /// From gear (`ReviveOnce`); decremented when a revival triggers.
    #[serde(default)]
    pub revive_charges: u8,
    /// From gear (`ReviveAtBackOnce`); decremented when this revival triggers.
    #[serde(default)]
    pub revive_at_back_charges: u8,
    #[serde(default)]
    pub applied_per_ally_might: i32,
    #[serde(default)]
    pub applied_per_ally_reflexes: i32,
    #[serde(default)]
    pub applied_per_ally_wisdom: i32,
    /// Like `formation_hp_bonus`: extra HP cap from the per-ally aura. Included in `effective_max_hp`.
    #[serde(default)]
    pub per_ally_hp_bonus: i32,
    /// Healers (`Property::Healer`) start battle with [`HEALER_MAX_MANA`] and spend 1 per heal.
    #[serde(default)]
    pub mana: i32,
    #[serde(default)]
    pub max_mana: i32,
}

pub const HEALER_MAX_MANA: i32 = 20;

#[inline]
fn healer_mana_from_props(props: &[Property]) -> (i32, i32) {
    let cap = props
        .iter()
        .any(|p| matches!(p, Property::Healer))
        .then_some(HEALER_MAX_MANA)
        .unwrap_or(0);
    (cap, cap)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CombatEvent {
    Start {
        left: Vec<Combatant>,
        right: Vec<Combatant>,
    },
    Attack {
        attacker: u32,
        target: u32,
        ranged: bool,
        projectile: Option<String>,
        damage: i32,
        hit: bool,
        #[serde(default)]
        critical: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        simultaneous_group: Option<u32>,
    },
    Heal {
        healer: u32,
        target: u32,
        amount: i32,
    },
    /// Current mana after a heal or other mana change (`Property::Healer` units only).
    Mana {
        uid: u32,
        mana: i32,
    },
    Freeze {
        target: u32,
        sprite: String,
    },
    Death {
        uid: u32,
        side: u8,
    },
    DeathBlast {
        source: u32,
        target: u32,
        damage: i32,
    },
    Revive {
        uid: u32,
        hp: i32,
    },
    /// Wearer of a propellor hat reaches 0 HP and zips to the back of formation at full HP.
    ReviveAtBack {
        uid: u32,
        hp: i32,
    },
    Summon {
        side: u8,
        summoner: u32,
        combatant: Combatant,
    },
    /// Client sync after formation-front aura totals change.
    StatSync {
        uid: u32,
        might: i32,
        reflexes: i32,
        wisdom: i32,
        max_hp: i32,
        hp: i32,
        formation_hp_bonus: i32,
        #[serde(default)]
        applied_front_might: i32,
        #[serde(default)]
        applied_front_reflexes: i32,
        #[serde(default)]
        applied_front_wisdom: i32,
        #[serde(default)]
        applied_enemy_reflex_debuff: i32,
        #[serde(default)]
        per_ally_hp_bonus: i32,
        #[serde(default)]
        applied_per_ally_might: i32,
        #[serde(default)]
        applied_per_ally_reflexes: i32,
        #[serde(default)]
        applied_per_ally_wisdom: i32,
    },
    /// Peels `amount` from might, reflexes, wisdom, max HP, and current HP ([`Property::DrainEnemyStatsOnHit`]).
    StatDrain {
        uid: u32,
        amount: i32,
    },
    Hp {
        uid: u32,
        hp: i32,
    },
    LockBreaker {
        winner: Option<u8>,
        left_living_gold: i32,
        right_living_gold: i32,
        left_living_units: usize,
        right_living_units: usize,
        left_hp: i32,
        right_hp: i32,
    },
    End {
        winner: Option<u8>,
    }, // None = draw
}

pub struct BattleResult {
    pub events: Vec<CombatEvent>,
    pub winner: Option<u8>, // None = draw, Some(0) left, Some(1) right
}

const MAX_BATTLE_TURNS: usize = 10_000;

fn build_team(build: &Build, side: u8, uid_start: &mut u32) -> Vec<Combatant> {
    build
        .team
        .iter()
        .filter_map(|m| {
            let def = character_def(&m.def_id)?;
            let mut might = def.might;
            let mut reflexes = def.reflexes;
            let mut wisdom = def.wisdom;
            let mut hp = def.hp;
            let mut props = def.properties.clone();
            let mut hat_sprite = None;
            let mut left_hand_sprite = None;
            let mut right_hand_sprite = None;
            let mut hand_3_sprite = None;
            let mut hand_4_sprite = None;
            let mut hat_id = None;
            let mut left_hand_id = None;
            let mut right_hand_id = None;
            let mut hand_3_id = None;
            let mut hand_4_id = None;
            for (slot_id, sprite_out, id_out) in [
                (&m.hat, &mut hat_sprite, &mut hat_id),
                (&m.left_hand, &mut left_hand_sprite, &mut left_hand_id),
                (&m.right_hand, &mut right_hand_sprite, &mut right_hand_id),
                (&m.hand_3, &mut hand_3_sprite, &mut hand_3_id),
                (&m.hand_4, &mut hand_4_sprite, &mut hand_4_id),
            ] {
                if let Some(iid) = slot_id {
                    *id_out = Some(iid.clone());
                    if let Some(idef) = item_def(iid) {
                        *sprite_out = Some(idef.sprite.clone());
                        for p in &idef.properties {
                            if let Property::StatBonus {
                                might: m_,
                                reflexes: r_,
                                wisdom: w_,
                                hp: h_,
                            } = p
                            {
                                might += m_;
                                reflexes += r_;
                                wisdom += w_;
                                hp += h_;
                            } else {
                                props.push(p.clone());
                            }
                        }
                    }
                }
            }
            let revive_charges = props
                .iter()
                .filter(|p| matches!(p, Property::ReviveOnce))
                .count() as u8;
            let revive_at_back_charges = props
                .iter()
                .filter(|p| matches!(p, Property::ReviveAtBackOnce))
                .count() as u8;
            props.retain(|p| !matches!(p, Property::ReviveOnce | Property::ReviveAtBackOnce));
            let (mana, max_mana) = healer_mana_from_props(&props);
            let uid = *uid_start;
            *uid_start += 1;
            Some(Combatant {
                uid,
                def_id: def.id.clone(),
                sprite: def.sprite.clone(),
                hat_id,
                left_hand_id,
                right_hand_id,
                hand_3_id,
                hand_4_id,
                hat_sprite,
                left_hand_sprite,
                right_hand_sprite,
                hand_3_sprite,
                hand_4_sprite,
                max_hp: hp,
                hp,
                might,
                reflexes,
                wisdom,
                properties: props,
                frozen_turns: 0,
                side,
                applied_front_might: 0,
                applied_front_reflexes: 0,
                applied_front_wisdom: 0,
                formation_hp_bonus: 0,
                applied_enemy_reflex_debuff: 0,
                revive_charges,
                revive_at_back_charges,
                applied_per_ally_might: 0,
                applied_per_ally_reflexes: 0,
                applied_per_ally_wisdom: 0,
                per_ally_hp_bonus: 0,
                mana,
                max_mana,
            })
        })
        .collect()
}

#[inline]
fn effective_max_hp(c: &Combatant) -> i32 {
    c.max_hp + c.formation_hp_bonus + c.per_ally_hp_bonus
}

fn ally_front_formation_bonus(team: &[Combatant]) -> (i32, i32, i32, i32) {
    let mut s = (0i32, 0i32, 0i32, 0i32);
    for c in team {
        if c.hp <= 0 {
            continue;
        }
        for p in &c.properties {
            if let Property::BuffFormationFront {
                might,
                reflexes,
                wisdom,
                hp,
            } = p
            {
                s.0 += might;
                s.1 += reflexes;
                s.2 += wisdom;
                s.3 += hp;
            }
        }
    }
    s
}

fn refresh_formation_front_aura(team: &mut [Combatant], mut events: Option<&mut Vec<CombatEvent>>) {
    let bonus = ally_front_formation_bonus(team);
    let front_idx = first_alive_idx(team);
    for (i, c) in team.iter_mut().enumerate() {
        if c.hp <= 0 {
            continue;
        }

        let want = if front_idx == Some(i) {
            bonus
        } else {
            (0, 0, 0, 0)
        };

        let intrinsic_m = c.might - c.applied_front_might;
        let intrinsic_r = c.reflexes - c.applied_front_reflexes;
        let intrinsic_w = c.wisdom - c.applied_front_wisdom;

        let new_might = intrinsic_m + want.0;
        let new_refl = intrinsic_r + want.1;
        let new_wis = intrinsic_w + want.2;

        let dm = new_might - c.might;
        let dr = new_refl - c.reflexes;
        let dw = new_wis - c.wisdom;

        let old_fhb = c.formation_hp_bonus;
        let new_fhb = want.3;
        let dh = new_fhb - old_fhb;

        let changed = dm != 0 || dr != 0 || dw != 0 || dh != 0;

        c.might = new_might;
        c.reflexes = new_refl;
        c.wisdom = new_wis;
        c.applied_front_might = want.0;
        c.applied_front_reflexes = want.1;
        c.applied_front_wisdom = want.2;

        c.formation_hp_bonus = new_fhb;
        let eff_max = effective_max_hp(c);
        // Gaining aura HP bonus: add that pool like a heal (capped at new effective max).
        // Losing it: don't subtract dh from current hp — only clamp to the new ceiling so we
        // don't "damage" twice when another ally becomes formation front with the same aura.
        if dh > 0 {
            c.hp = (c.hp + dh).min(eff_max);
        } else {
            c.hp = c.hp.min(eff_max);
        }
        c.hp = c.hp.max(0);

        if changed {
            if let Some(ev) = events.as_mut() {
                ev.push(CombatEvent::StatSync {
                    uid: c.uid,
                    might: c.might,
                    reflexes: c.reflexes,
                    wisdom: c.wisdom,
                    max_hp: c.max_hp,
                    hp: c.hp,
                    formation_hp_bonus: c.formation_hp_bonus,
                    applied_front_might: c.applied_front_might,
                    applied_front_reflexes: c.applied_front_reflexes,
                    applied_front_wisdom: c.applied_front_wisdom,
                    applied_enemy_reflex_debuff: c.applied_enemy_reflex_debuff,
                    per_ally_hp_bonus: c.per_ally_hp_bonus,
                    applied_per_ally_might: c.applied_per_ally_might,
                    applied_per_ally_reflexes: c.applied_per_ally_reflexes,
                    applied_per_ally_wisdom: c.applied_per_ally_wisdom,
                });
            }
        }
    }
}

fn refresh_per_ally_aura(team: &mut [Combatant], mut events: Option<&mut Vec<CombatEvent>>) {
    let alive = team.iter().filter(|c| c.hp > 0).count() as i32;
    for c in team.iter_mut() {
        if c.hp <= 0 {
            // Strip applied bonus on death so accounting stays consistent.
            let stripped = c.applied_per_ally_might != 0
                || c.applied_per_ally_reflexes != 0
                || c.applied_per_ally_wisdom != 0
                || c.per_ally_hp_bonus != 0;
            c.might -= c.applied_per_ally_might;
            c.reflexes -= c.applied_per_ally_reflexes;
            c.wisdom -= c.applied_per_ally_wisdom;
            c.applied_per_ally_might = 0;
            c.applied_per_ally_reflexes = 0;
            c.applied_per_ally_wisdom = 0;
            c.per_ally_hp_bonus = 0;
            if stripped {
                // Dead unit: no need to clamp hp (already 0) or emit a sync.
            }
            continue;
        }
        let per: i32 = c
            .properties
            .iter()
            .filter_map(|p| {
                if let Property::StatsPerLivingAlly { amount } = p {
                    Some(*amount)
                } else {
                    None
                }
            })
            .sum();
        let others = (alive - 1).max(0);
        let want = per * others;
        if want == c.applied_per_ally_might
            && want == c.applied_per_ally_reflexes
            && want == c.applied_per_ally_wisdom
            && want == c.per_ally_hp_bonus
        {
            continue;
        }

        let intrinsic_m = c.might - c.applied_per_ally_might;
        let intrinsic_r = c.reflexes - c.applied_per_ally_reflexes;
        let intrinsic_w = c.wisdom - c.applied_per_ally_wisdom;
        c.might = intrinsic_m + want;
        c.reflexes = intrinsic_r + want;
        c.wisdom = intrinsic_w + want;
        c.applied_per_ally_might = want;
        c.applied_per_ally_reflexes = want;
        c.applied_per_ally_wisdom = want;

        let old_hp_bonus = c.per_ally_hp_bonus;
        c.per_ally_hp_bonus = want;
        let dh = want - old_hp_bonus;
        let eff_max = effective_max_hp(c);
        // Mirror BuffFormationFront's hp handling: gain heals, loss only clamps.
        if dh > 0 {
            c.hp = (c.hp + dh).min(eff_max);
        } else {
            c.hp = c.hp.min(eff_max);
        }
        c.hp = c.hp.max(0);

        if let Some(ev) = events.as_mut() {
            push_stat_sync(c, ev);
        }
    }
}

#[inline]
fn armour_total(props: &[Property]) -> i32 {
    props
        .iter()
        .filter_map(|p| {
            if let Property::Armour { value } = p {
                Some(*value)
            } else {
                None
            }
        })
        .sum()
}

/// Raw damage after crit; defender’s total armour subtracts flat per hit (min 0).
#[inline]
fn damage_after_armour(raw: i32, defender: &Combatant) -> i32 {
    if raw <= 0 {
        return 0;
    }
    let red = armour_total(&defender.properties);
    (raw - red).max(0)
}

/// Sum of [`Property::DebuffEnemyReflexes`] from living units on the attacking side (applied to defenders' reflexes for hit rolls).
fn debuff_enemy_reflex_total(attackers: &[Combatant]) -> i32 {
    attackers
        .iter()
        .filter(|c| c.hp > 0)
        .flat_map(|c| c.properties.iter())
        .filter_map(|p| {
            if let Property::DebuffEnemyReflexes { amount } = p {
                Some(*amount)
            } else {
                None
            }
        })
        .sum()
}

fn refresh_enemy_reflex_debuffs(
    left: &mut [Combatant],
    right: &mut [Combatant],
    mut events: Option<&mut Vec<CombatEvent>>,
) {
    let left_debuff = debuff_enemy_reflex_total(left);
    let right_debuff = debuff_enemy_reflex_total(right);

    for c in left.iter_mut() {
        if c.hp <= 0 {
            continue;
        }
        if c.applied_enemy_reflex_debuff != right_debuff {
            c.applied_enemy_reflex_debuff = right_debuff;
            if let Some(ev) = events.as_mut() {
                push_stat_sync(c, ev);
            }
        }
    }
    for c in right.iter_mut() {
        if c.hp <= 0 {
            continue;
        }
        if c.applied_enemy_reflex_debuff != left_debuff {
            c.applied_enemy_reflex_debuff = left_debuff;
            if let Some(ev) = events.as_mut() {
                push_stat_sync(c, ev);
            }
        }
    }
}

/// Hit chance vs the defender. Same formula for melee and ranged.
fn hit_chance(att: &Combatant, def: &Combatant) -> f32 {
    let def_r = def
        .reflexes
        .saturating_sub(def.applied_enemy_reflex_debuff)
        .max(0);
    let diff = (att.reflexes - def_r) as f32;
    (0.7 + diff * 0.03).clamp(0.1, 0.95)
}

/// After a successful hit, roll crit from attacker's properties (first matching `CritStrike`).
fn roll_crit_damage(base: i32, props: &[Property], rng: &mut Rng) -> (i32, bool) {
    if base <= 0 {
        return (base, false);
    }
    let Some(pct) = props.iter().find_map(|p| {
        if let Property::CritStrike { chance_percent } = p {
            Some(*chance_percent)
        } else {
            None
        }
    }) else {
        return (base, false);
    };
    let p = (pct as f32 / 100.0).clamp(0.0, 1.0);
    if rng.chance(p) {
        (base.saturating_mul(2), true)
    } else {
        (base, false)
    }
}

fn first_alive_idx(team: &[Combatant]) -> Option<usize> {
    team.iter().position(|c| c.hp > 0)
}

/// 0 = front (first living in formation order), 1 = second living, etc.
fn formation_alive_rank(team: &[Combatant], idx: usize) -> Option<usize> {
    if idx >= team.len() || team[idx].hp <= 0 {
        return None;
    }
    let mut rank = 0usize;
    for (j, c) in team.iter().enumerate() {
        if c.hp <= 0 {
            continue;
        }
        if j == idx {
            return Some(rank);
        }
        rank += 1;
    }
    None
}

fn first_damaged_idx(team: &[Combatant], excluding: u32) -> Option<usize> {
    team.iter()
        .enumerate()
        .filter(|(_, c)| c.hp > 0 && c.hp < effective_max_hp(c) && c.uid != excluding)
        .min_by_key(|(_, c)| effective_max_hp(c) - c.hp) // any damaged; pick least-damaged so heals top up
        .map(|(i, _)| i)
}

fn living_gold(team: &[Combatant]) -> i32 {
    team.iter()
        .filter(|c| c.hp > 0)
        .map(|c| {
            let mut cost = character_def(&c.def_id).map(|d| d.cost).unwrap_or(0);
            for item_id in [
                &c.hat_id,
                &c.left_hand_id,
                &c.right_hand_id,
                &c.hand_3_id,
                &c.hand_4_id,
            ]
                .into_iter()
                .flatten()
            {
                cost += item_def(item_id).map(|d| d.cost).unwrap_or(0);
            }
            cost
        })
        .sum()
}

fn living_units(team: &[Combatant]) -> usize {
    team.iter().filter(|c| c.hp > 0).count()
}

fn living_hp(team: &[Combatant]) -> i32 {
    team.iter().filter(|c| c.hp > 0).map(|c| c.hp.max(0)).sum()
}

fn lock_break_winner(
    left: &[Combatant],
    right: &[Combatant],
) -> (Option<u8>, i32, i32, usize, usize, i32, i32) {
    let left_gold = living_gold(left);
    let right_gold = living_gold(right);
    let left_units = living_units(left);
    let right_units = living_units(right);
    let left_hp = living_hp(left);
    let right_hp = living_hp(right);
    let winner = left_gold
        .cmp(&right_gold)
        .then(left_units.cmp(&right_units))
        .then(left_hp.cmp(&right_hp));
    let winner = match winner {
        std::cmp::Ordering::Greater => Some(0),
        std::cmp::Ordering::Less => Some(1),
        std::cmp::Ordering::Equal => None,
    };
    (
        winner,
        left_gold,
        right_gold,
        left_units,
        right_units,
        left_hp,
        right_hp,
    )
}

fn summon_on_enemy_death(
    actors: &mut Vec<Combatant>,
    dead_def: &str,
    attacker_side: u8,
    events: &mut Vec<CombatEvent>,
    uid_counter: &mut u32,
) {
    let mut summons: Vec<(Combatant, u32)> = vec![];
    for ally in actors.iter() {
        if ally.hp <= 0 {
            continue;
        }
        for p in &ally.properties {
            if let Property::SummonOnEnemyDeath { species } = p {
                if dead_def != species.as_str()
                    && actors.iter().filter(|c| c.hp > 0).count() + summons.len() < MAX_TEAM
                {
                    if let Some(def) = character_def(species) {
                        let uid = *uid_counter;
                        *uid_counter += 1;
                        let (mana, max_mana) = healer_mana_from_props(&def.properties);
                        summons.push((
                            Combatant {
                                uid,
                                def_id: def.id.clone(),
                                sprite: def.sprite.clone(),
                                hat_id: None,
                                left_hand_id: None,
                                right_hand_id: None,
                                hand_3_id: None,
                                hand_4_id: None,
                                hat_sprite: None,
                                left_hand_sprite: None,
                                right_hand_sprite: None,
                                hand_3_sprite: None,
                                hand_4_sprite: None,
                                max_hp: def.hp,
                                hp: def.hp,
                                might: def.might,
                                reflexes: def.reflexes,
                                wisdom: def.wisdom,
                                properties: def.properties.clone(),
                                frozen_turns: 0,
                                side: attacker_side,
                                applied_front_might: 0,
                                applied_front_reflexes: 0,
                                applied_front_wisdom: 0,
                                formation_hp_bonus: 0,
                                applied_enemy_reflex_debuff: 0,
                                revive_charges: 0,
                                revive_at_back_charges: 0,
                                applied_per_ally_might: 0,
                                applied_per_ally_reflexes: 0,
                                applied_per_ally_wisdom: 0,
                                per_ally_hp_bonus: 0,
                                mana,
                                max_mana,
                            },
                            ally.uid,
                        ));
                    }
                }
            }
        }
    }
    for (s, summoner_uid) in summons {
        events.push(CombatEvent::Summon {
            side: attacker_side,
            summoner: summoner_uid,
            combatant: s.clone(),
        });
        if let Some(idx) = actors.iter().position(|c| c.uid == summoner_uid) {
            actors.insert(idx, s);
        } else {
            actors.push(s);
        }
    }
}

fn summon_on_ally_death(
    foes: &mut Vec<Combatant>,
    dead_def: &str,
    dead_side: u8,
    events: &mut Vec<CombatEvent>,
    uid_counter: &mut u32,
) {
    let mut ally_summons: Vec<(Combatant, u32)> = vec![];
    for ally in foes.iter() {
        if ally.hp <= 0 {
            continue;
        }
        for p in &ally.properties {
            if let Property::SummonOnAllyDeath { species } = p {
                if dead_def != species.as_str()
                    && foes.iter().filter(|c| c.hp > 0).count() + ally_summons.len() < MAX_TEAM
                {
                    if let Some(def) = character_def(species) {
                        let uid = *uid_counter;
                        *uid_counter += 1;
                        let (mana, max_mana) = healer_mana_from_props(&def.properties);
                        ally_summons.push((
                            Combatant {
                                uid,
                                def_id: def.id.clone(),
                                sprite: def.sprite.clone(),
                                hat_id: None,
                                left_hand_id: None,
                                right_hand_id: None,
                                hand_3_id: None,
                                hand_4_id: None,
                                hat_sprite: None,
                                left_hand_sprite: None,
                                right_hand_sprite: None,
                                hand_3_sprite: None,
                                hand_4_sprite: None,
                                max_hp: def.hp,
                                hp: def.hp,
                                might: def.might,
                                reflexes: def.reflexes,
                                wisdom: def.wisdom,
                                properties: def.properties.clone(),
                                frozen_turns: 0,
                                side: dead_side,
                                applied_front_might: 0,
                                applied_front_reflexes: 0,
                                applied_front_wisdom: 0,
                                formation_hp_bonus: 0,
                                applied_enemy_reflex_debuff: 0,
                                revive_charges: 0,
                                revive_at_back_charges: 0,
                                applied_per_ally_might: 0,
                                applied_per_ally_reflexes: 0,
                                applied_per_ally_wisdom: 0,
                                per_ally_hp_bonus: 0,
                                mana,
                                max_mana,
                            },
                            ally.uid,
                        ));
                    }
                }
            }
        }
    }
    for (s, summoner_uid) in ally_summons {
        events.push(CombatEvent::Summon {
            side: dead_side,
            summoner: summoner_uid,
            combatant: s.clone(),
        });
        if let Some(idx) = foes.iter().position(|c| c.uid == summoner_uid) {
            foes.insert(idx, s);
        } else {
            foes.push(s);
        }
    }
}

fn push_stat_sync(c: &Combatant, events: &mut Vec<CombatEvent>) {
    events.push(CombatEvent::StatSync {
        uid: c.uid,
        might: c.might,
        reflexes: c.reflexes,
        wisdom: c.wisdom,
        max_hp: c.max_hp,
        hp: c.hp,
        formation_hp_bonus: c.formation_hp_bonus,
        applied_front_might: c.applied_front_might,
        applied_front_reflexes: c.applied_front_reflexes,
        applied_front_wisdom: c.applied_front_wisdom,
        applied_enemy_reflex_debuff: c.applied_enemy_reflex_debuff,
        per_ally_hp_bonus: c.per_ally_hp_bonus,
        applied_per_ally_might: c.applied_per_ally_might,
        applied_per_ally_reflexes: c.applied_per_ally_reflexes,
        applied_per_ally_wisdom: c.applied_per_ally_wisdom,
    });
}

/// Subtract HP damage.
fn apply_damage_to_target(target: &mut Combatant, damage: i32) {
    if damage <= 0 {
        return;
    }
    target.hp -= damage;
}

#[inline]
fn drain_enemy_stats_on_hit_amount(props: &[Property]) -> i32 {
    props
        .iter()
        .filter_map(|p| {
            if let Property::DrainEnemyStatsOnHit { amount } = p {
                Some(*amount)
            } else {
                None
            }
        })
        .sum()
}

/// Peel `amount` from might/reflexes/wisdom, reduce max HP (min 1), clamp HP to effective max.
fn apply_flat_stat_damage_to_target(
    target: &mut Combatant,
    amount: i32,
    events: &mut Vec<CombatEvent>,
) {
    if amount <= 0 {
        return;
    }
    target.might = (target.might - amount).max(0);
    target.reflexes = (target.reflexes - amount).max(0);
    target.wisdom = (target.wisdom - amount).max(0);
    target.max_hp = (target.max_hp - amount).max(1);
    let cap = effective_max_hp(target);
    target.hp = (target.hp - amount).max(0).min(cap);
    events.push(CombatEvent::StatDrain {
        uid: target.uid,
        amount,
    });
    push_stat_sync(target, events);
}

fn handle_unit_dropped(
    victims: &mut Vec<Combatant>,
    enemies: &mut Vec<Combatant>,
    victim_idx: usize,
    killer_uid: u32,
    killer_side: u8,
    events: &mut Vec<CombatEvent>,
    uid_counter: &mut u32,
) {
    let dead_uid = victims[victim_idx].uid;
    let dead_def = victims[victim_idx].def_id.clone();
    let dead_side = victims[victim_idx].side;
    let dead_might = victims[victim_idx].might;
    let dead_props = victims[victim_idx].properties.clone();
    events.push(CombatEvent::Death {
        uid: dead_uid,
        side: dead_side,
    });
    apply_ally_death_bonuses(victims, events);
    apply_team_stats_on_death(victims, &dead_props, events);
    apply_kill_bonuses(enemies, killer_uid, events);
    apply_damage_enemy_on_death(
        enemies,
        victims,
        dead_uid,
        dead_might,
        &dead_props,
        dead_side,
        events,
        uid_counter,
    );
    summon_on_enemy_death(enemies, &dead_def, killer_side, events, uid_counter);
    summon_on_ally_death(victims, &dead_def, dead_side, events, uid_counter);
}

fn resolve_dropped_unit(
    victims: &mut Vec<Combatant>,
    enemies: &mut Vec<Combatant>,
    victim_idx: usize,
    killer_uid: u32,
    killer_side: u8,
    events: &mut Vec<CombatEvent>,
    uid_counter: &mut u32,
) -> bool {
    if victim_idx >= victims.len() {
        return false;
    }

    let victim_uid = victims[victim_idx].uid;
    if victims[victim_idx].hp > 0 {
        events.push(CombatEvent::Hp {
            uid: victim_uid,
            hp: victims[victim_idx].hp,
        });
        return false;
    }

    let will_revive = victims[victim_idx].revive_charges > 0;
    let will_revive_at_back =
        !will_revive && victims[victim_idx].revive_at_back_charges > 0;
    events.push(CombatEvent::Hp {
        uid: victim_uid,
        hp: 0,
    });
    handle_unit_dropped(
        victims,
        enemies,
        victim_idx,
        killer_uid,
        killer_side,
        events,
        uid_counter,
    );

    if will_revive {
        if let Some(idx) = victims.iter().position(|c| c.uid == victim_uid) {
            victims[idx].revive_charges = victims[idx].revive_charges.saturating_sub(1);
            victims[idx].hp = effective_max_hp(&victims[idx]);
            events.push(CombatEvent::Revive {
                uid: victim_uid,
                hp: victims[idx].hp,
            });
            events.push(CombatEvent::Hp {
                uid: victim_uid,
                hp: victims[idx].hp,
            });
        }
    } else if will_revive_at_back {
        if let Some(idx) = victims.iter().position(|c| c.uid == victim_uid) {
            victims[idx].revive_at_back_charges =
                victims[idx].revive_at_back_charges.saturating_sub(1);
            victims[idx].hp = effective_max_hp(&victims[idx]);
            let hp = victims[idx].hp;
            // Reposition to the back of the formation.
            let unit = victims.remove(idx);
            victims.push(unit);
            events.push(CombatEvent::ReviveAtBack {
                uid: victim_uid,
                hp,
            });
            events.push(CombatEvent::Hp {
                uid: victim_uid,
                hp,
            });
        }
    }

    true
}

fn apply_kill_bonuses(actors: &mut [Combatant], killer_uid: u32, events: &mut Vec<CombatEvent>) {
    let Some(c) = actors.iter_mut().find(|c| c.uid == killer_uid) else {
        return;
    };
    if c.hp <= 0 {
        return;
    }
    let mut dm = 0i32;
    let mut dr = 0i32;
    let mut dw = 0i32;
    let mut dhp = 0i32;
    for p in &c.properties {
        if let Property::StatsOnKill {
            might: m,
            reflexes: r,
            wisdom: w,
            hp: h,
        } = p
        {
            dm += *m;
            dr += *r;
            dw += *w;
            dhp += *h;
        }
    }
    if dm == 0 && dr == 0 && dw == 0 && dhp == 0 {
        return;
    }
    c.might += dm;
    c.reflexes += dr;
    c.wisdom += dw;
    c.max_hp += dhp;
    if dhp != 0 {
        let cap = effective_max_hp(c);
        c.hp = (c.hp + dhp).clamp(0, cap);
    }
    push_stat_sync(c, events);
}

/// Apply item effects that trigger when an ally dies (`foes` is that ally's team).
fn apply_ally_death_bonuses(foes: &mut [Combatant], events: &mut Vec<CombatEvent>) {
    for c in foes.iter_mut() {
        if c.hp <= 0 {
            continue;
        }
        let mut dm = 0i32;
        let mut dr = 0i32;
        let mut dw = 0i32;
        let mut dhp = 0i32;
        for p in &c.properties {
            match p {
                Property::MightOnAllyDeath { might } => dm += *might,
                Property::StatsOnAllyDeath {
                    might: m,
                    reflexes: r,
                    wisdom: w,
                    hp: h,
                } => {
                    dm += *m;
                    dr += *r;
                    dw += *w;
                    dhp += *h;
                }
                _ => {}
            }
        }
        if dm == 0 && dr == 0 && dw == 0 && dhp == 0 {
            continue;
        }
        c.might += dm;
        c.reflexes += dr;
        c.wisdom += dw;
        c.max_hp += dhp;
        if dhp != 0 {
            let cap = effective_max_hp(c);
            c.hp = (c.hp + dhp).clamp(0, cap);
        }
        push_stat_sync(c, events);
    }
}

fn apply_team_stats_on_death(
    team: &mut [Combatant],
    dead_props: &[Property],
    events: &mut Vec<CombatEvent>,
) {
    let bonus: i32 = dead_props
        .iter()
        .filter_map(|p| {
            if let Property::TeamStatsOnDeath { amount } = p {
                Some(*amount)
            } else {
                None
            }
        })
        .sum();
    if bonus == 0 {
        return;
    }

    for c in team.iter_mut() {
        if c.hp <= 0 {
            continue;
        }
        c.might += bonus;
        c.reflexes += bonus;
        c.wisdom += bonus;
        c.max_hp += bonus;
        let cap = effective_max_hp(c);
        c.hp = (c.hp + bonus).clamp(0, cap);
        push_stat_sync(c, events);
    }
}

fn apply_damage_enemy_on_death(
    enemies: &mut Vec<Combatant>,
    allies: &mut Vec<Combatant>,
    source_uid: u32,
    source_might: i32,
    source_props: &[Property],
    source_side: u8,
    events: &mut Vec<CombatEvent>,
    uid_counter: &mut u32,
) {
    let multiplier: i32 = source_props
        .iter()
        .filter_map(|p| {
            if let Property::DamageEnemyOnDeath { might_multiplier } = p {
                Some(*might_multiplier)
            } else {
                None
            }
        })
        .sum();
    let damage = source_might * multiplier;
    if damage <= 0 {
        return;
    }

    let Some(target_idx) = first_alive_idx(enemies) else {
        return;
    };
    let target_uid = enemies[target_idx].uid;
    events.push(CombatEvent::DeathBlast {
        source: source_uid,
        target: target_uid,
        damage,
    });
    enemies[target_idx].hp -= damage;
    resolve_dropped_unit(
        enemies,
        allies,
        target_idx,
        source_uid,
        source_side,
        events,
        uid_counter,
    );
}

pub fn resolve_battle(left_build: &Build, right_build: &Build, rng: &mut Rng) -> BattleResult {
    let mut uid_counter: u32 = 1;
    let mut left = build_team(left_build, 0, &mut uid_counter);
    let mut right = build_team(right_build, 1, &mut uid_counter);
    refresh_formation_front_aura(&mut left, None);
    refresh_formation_front_aura(&mut right, None);
    refresh_per_ally_aura(&mut left, None);
    refresh_per_ally_aura(&mut right, None);
    refresh_enemy_reflex_debuffs(&mut left, &mut right, None);
    let mut events = vec![CombatEvent::Start {
        left: left.clone(),
        right: right.clone(),
    }];
    let mut simultaneous_group_counter = 1u32;

    let mut capped = true;
    // Safety: cap turns to avoid infinite loops.
    for _turn in 0..MAX_BATTLE_TURNS {
        if left.iter().all(|c| c.hp <= 0) || right.iter().all(|c| c.hp <= 0) {
            break;
        }

        // Each side acts once per tick; collect actions for both, apply afterward.
        // But to keep things simple we alternate: left side then right side, killing in real-time.
        for side in 0..2u8 {
            let (actors, foes) = if side == 0 {
                (
                    &mut left as *mut Vec<Combatant>,
                    &mut right as *mut Vec<Combatant>,
                )
            } else {
                (
                    &mut right as *mut Vec<Combatant>,
                    &mut left as *mut Vec<Combatant>,
                )
            };
            // Iterate over the actors that existed at tick start; summons join the
            // formation immediately, but don't act until the next tick.
            let actor_uids: Vec<u32> = unsafe { (&*actors).iter().map(|c| c.uid).collect() };
            for actor_uid in actor_uids {
                let mut needs_aura = false;
                {
                    let actors = unsafe { &mut *actors };
                    let foes = unsafe { &mut *foes };

                    let Some(i) = actors.iter().position(|c| c.uid == actor_uid) else {
                        continue;
                    };
                    if actors[i].hp <= 0 {
                        continue;
                    }
                    if actors[i].frozen_turns > 0 {
                        actors[i].frozen_turns -= 1;
                        continue;
                    }
                    let rank = formation_alive_rank(actors, i);
                    let has_melee_from_second = actors[i]
                        .properties
                        .iter()
                        .any(|p| matches!(p, Property::MeleeFromSecond));
                    let can_melee = match rank {
                        Some(0) => true,
                        Some(1) => has_melee_from_second,
                        _ => false,
                    };
                    let has_ranged = actors[i]
                        .properties
                        .iter()
                        .any(|p| matches!(p, Property::Ranged { .. }));
                    let is_healer = actors[i]
                        .properties
                        .iter()
                        .any(|p| matches!(p, Property::Healer));

                    // Healer: heal frontmost damaged friendly (not self) if any mana, else attack.
                    if is_healer {
                        if actors[i].mana >= 1 {
                            if let Some(target_idx) = first_damaged_idx(actors, actors[i].uid) {
                                actors[i].mana -= 1;
                                let amount = actors[i].wisdom;
                                let cap = effective_max_hp(&actors[target_idx]);
                                actors[target_idx].hp = (actors[target_idx].hp + amount).min(cap);
                                events.push(CombatEvent::Heal {
                                    healer: actors[i].uid,
                                    target: actors[target_idx].uid,
                                    amount,
                                });
                                events.push(CombatEvent::Hp {
                                    uid: actors[target_idx].uid,
                                    hp: actors[target_idx].hp,
                                });
                                events.push(CombatEvent::Mana {
                                    uid: actors[i].uid,
                                    mana: actors[i].mana,
                                });
                                continue;
                            }
                        }
                    }

                    // Attack: first rank or (second rank with reach)=melee; else ranged if able.
                    if first_alive_idx(foes).is_none() {
                        break;
                    }
                    let (ranged, projectile, damage_stat) = if can_melee {
                        (false, None, actors[i].might)
                    } else if has_ranged {
                        let proj = actors[i].properties.iter().find_map(|p| {
                            if let Property::Ranged { projectile } = p {
                                Some(projectile.clone())
                            } else {
                                None
                            }
                        });
                        (true, proj, actors[i].wisdom)
                    } else {
                        continue;
                    };

                    let cleave_count = if !ranged {
                        let base = actors[i]
                            .properties
                            .iter()
                            .filter_map(|p| {
                                if let Property::MeleeCleave { count } = p {
                                    Some((*count).max(1) as usize)
                                } else {
                                    None
                                }
                            })
                            .max()
                            .unwrap_or(1);
                        let bonus: usize = actors[i]
                            .properties
                            .iter()
                            .filter_map(|p| {
                                if let Property::MeleeCleaveBonus { plus } = p {
                                    Some(*plus as usize)
                                } else {
                                    None
                                }
                            })
                            .sum();
                        base + bonus
                    } else {
                        1
                    };

                    let foe_uids: Vec<u32> = foes
                        .iter()
                        .filter(|c| c.hp > 0)
                        .take(cleave_count)
                        .map(|c| c.uid)
                        .collect();
                    let simultaneous_group = if foe_uids.len() > 1 {
                        let group = simultaneous_group_counter;
                        simultaneous_group_counter += 1;
                        Some(group)
                    } else {
                        None
                    };
                    let attacker_snapshot = actors[i].clone();
                    let on_hit_stat_drain =
                        drain_enemy_stats_on_hit_amount(&attacker_snapshot.properties);

                    for &target_uid in &foe_uids {
                        let Some(foe_idx) =
                            foes.iter().position(|c| c.uid == target_uid && c.hp > 0)
                        else {
                            continue;
                        };
                        let chance = hit_chance(&attacker_snapshot, &foes[foe_idx]);
                        let hit = rng.chance(chance);
                        let (damage, critical) = if hit {
                            let raw = roll_crit_damage(
                                damage_stat,
                                &attacker_snapshot.properties,
                                rng,
                            );
                            (damage_after_armour(raw.0, &foes[foe_idx]), raw.1)
                        } else {
                            (0, false)
                        };
                        events.push(CombatEvent::Attack {
                            attacker: attacker_snapshot.uid,
                            target: target_uid,
                            ranged,
                            projectile: projectile.clone(),
                            damage,
                            hit,
                            critical,
                            simultaneous_group,
                        });
                        if hit {
                            let freeze_sprite = attacker_snapshot.properties.iter().find_map(|p| {
                                if let Property::FreezeOnHit { sprite } = p {
                                    Some(sprite.clone())
                                } else {
                                    None
                                }
                            });
                            apply_damage_to_target(&mut foes[foe_idx], damage);
                            if on_hit_stat_drain > 0 && foes[foe_idx].hp > 0 {
                                apply_flat_stat_damage_to_target(
                                    &mut foes[foe_idx],
                                    on_hit_stat_drain,
                                    &mut events,
                                );
                            }
                            let dropped = resolve_dropped_unit(
                                foes,
                                actors,
                                foe_idx,
                                attacker_snapshot.uid,
                                side,
                                &mut events,
                                &mut uid_counter,
                            );
                            needs_aura |= dropped;

                            if let Some(spr) = freeze_sprite {
                                if let Some(target_idx) =
                                    foes.iter().position(|c| c.uid == target_uid)
                                {
                                    if foes[target_idx].hp > 0 {
                                        foes[target_idx].frozen_turns = 1;
                                        events.push(CombatEvent::Freeze {
                                            target: target_uid,
                                            sprite: spr,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
                if needs_aura {
                    refresh_formation_front_aura(&mut left, Some(&mut events));
                    refresh_formation_front_aura(&mut right, Some(&mut events));
                    refresh_per_ally_aura(&mut left, Some(&mut events));
                    refresh_per_ally_aura(&mut right, Some(&mut events));
                    refresh_enemy_reflex_debuffs(&mut left, &mut right, Some(&mut events));
                }
            }
        }
        if left.iter().all(|c| c.hp <= 0) || right.iter().all(|c| c.hp <= 0) {
            capped = false;
            break;
        }
    }

    let left_alive = left.iter().any(|c| c.hp > 0);
    let right_alive = right.iter().any(|c| c.hp > 0);
    let mut winner = match (left_alive, right_alive) {
        (true, false) => Some(0),
        (false, true) => Some(1),
        _ => None,
    };
    if capped && left_alive && right_alive {
        let (
            lock_winner,
            left_living_gold,
            right_living_gold,
            left_living_units,
            right_living_units,
            left_hp,
            right_hp,
        ) = lock_break_winner(&left, &right);
        winner = lock_winner;
        events.push(CombatEvent::LockBreaker {
            winner,
            left_living_gold,
            right_living_gold,
            left_living_units,
            right_living_units,
            left_hp,
            right_hp,
        });
    }
    events.push(CombatEvent::End { winner });
    BattleResult { events, winner }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_combatant(
        uid: u32,
        def_id: &str,
        side: u8,
        hp: i32,
        wisdom: i32,
        properties: Vec<Property>,
    ) -> Combatant {
        Combatant {
            uid,
            def_id: def_id.into(),
            sprite: "test.webp".into(),
            hat_id: None,
            left_hand_id: None,
            right_hand_id: None,
            hand_3_id: None,
            hand_4_id: None,
            hat_sprite: None,
            left_hand_sprite: None,
            right_hand_sprite: None,
            hand_3_sprite: None,
            hand_4_sprite: None,
            max_hp: 20,
            hp,
            might: 1,
            reflexes: 1,
            wisdom,
            properties,
            frozen_turns: 0,
            side,
            applied_front_might: 0,
            applied_front_reflexes: 0,
            applied_front_wisdom: 0,
            formation_hp_bonus: 0,
            applied_enemy_reflex_debuff: 0,
            revive_charges: 0,
            revive_at_back_charges: 0,
            applied_per_ally_might: 0,
            applied_per_ally_reflexes: 0,
            applied_per_ally_wisdom: 0,
            per_ally_hp_bonus: 0,
            mana: 0,
            max_mana: 0,
        }
    }

    #[test]
    fn revived_units_still_trigger_vegetal_death_procs() {
        let mut victims = vec![
            test_combatant(1, "meme_man", 1, 0, 1, vec![]),
            test_combatant(
                2,
                "dark_vegetal",
                1,
                20,
                1,
                vec![Property::SummonOnAllyDeath {
                    species: "dark_vegetal".into(),
                }],
            ),
        ];
        victims[0].revive_charges = 1;
        let mut enemies = vec![test_combatant(
            3,
            "vegetal",
            0,
            20,
            1,
            vec![Property::SummonOnEnemyDeath {
                species: "vegetal".into(),
            }],
        )];
        let mut events = vec![];
        let mut uid_counter = 10;

        resolve_dropped_unit(
            &mut victims,
            &mut enemies,
            0,
            3,
            0,
            &mut events,
            &mut uid_counter,
        );

        assert_eq!(victims[0].hp, effective_max_hp(&victims[0]));
        assert!(events
            .iter()
            .any(|ev| matches!(ev, CombatEvent::Revive { uid: 1, .. })));
        assert!(events.iter().any(|ev| matches!(
            ev,
            CombatEvent::Summon { combatant, .. } if combatant.def_id == "vegetal"
        )));
        assert!(events.iter().any(|ev| matches!(
            ev,
            CombatEvent::Summon { combatant, .. } if combatant.def_id == "dark_vegetal"
        )));
    }

    #[test]
    fn revived_chillis_still_trigger_death_blast() {
        let mut victims = vec![test_combatant(
            1,
            "redchilli",
            1,
            0,
            5,
            vec![Property::DamageEnemyOnDeath {
                might_multiplier: 2,
            }],
        )];
        victims[0].might = 10;
        victims[0].revive_charges = 1;
        let mut enemies = vec![test_combatant(2, "meme_man", 0, 60, 1, vec![])];
        let mut events = vec![];
        let mut uid_counter = 10;

        resolve_dropped_unit(
            &mut victims,
            &mut enemies,
            0,
            2,
            0,
            &mut events,
            &mut uid_counter,
        );

        assert_eq!(victims[0].hp, effective_max_hp(&victims[0]));
        assert_eq!(enemies[0].hp, 40);
        assert!(events.iter().any(|ev| matches!(
            ev,
            CombatEvent::DeathBlast {
                source: 1,
                target: 2,
                damage: 20
            }
        )));
    }

    #[test]
    fn green_chilli_grants_flat_stats_on_death() {
        let mut victims = vec![
            test_combatant(
                1,
                "greenchilli",
                1,
                0,
                5,
                vec![Property::TeamStatsOnDeath { amount: 3 }],
            ),
            test_combatant(2, "meme_man", 1, 20, 1, vec![]),
        ];
        let mut enemies = vec![test_combatant(3, "meme_man", 0, 20, 1, vec![])];
        let mut events = vec![];
        let mut uid_counter = 10;

        resolve_dropped_unit(
            &mut victims,
            &mut enemies,
            0,
            3,
            0,
            &mut events,
            &mut uid_counter,
        );

        assert_eq!(victims[1].might, 4);
        assert_eq!(victims[1].reflexes, 4);
        assert_eq!(victims[1].wisdom, 4);
        assert_eq!(victims[1].max_hp, 23);
    }

    #[test]
    fn drain_enemy_stats_on_hit_peels_fixed_amount() {
        let mut c = test_combatant(1, "meme_man", 0, 20, 10, vec![]);
        c.might = 10;
        c.reflexes = 10;
        c.wisdom = 10;
        c.max_hp = 20;
        c.hp = 20;
        let mut events = vec![];
        apply_flat_stat_damage_to_target(&mut c, 1, &mut events);
        assert_eq!(c.might, 9);
        assert_eq!(c.reflexes, 9);
        assert_eq!(c.wisdom, 9);
        assert_eq!(c.max_hp, 19);
        assert_eq!(c.hp, 19);
        assert!(events
            .iter()
            .any(|e| matches!(e, CombatEvent::StatDrain { uid: 1, amount: 1 })));
        assert!(events
            .iter()
            .any(|e| matches!(e, CombatEvent::StatSync { uid: 1, .. })));
    }

    #[test]
    fn debuff_enemy_reflex_improves_hit_odds_for_attackers() {
        let mut att = test_combatant(1, "meme_man", 0, 20, 10, vec![]);
        att.reflexes = 10;
        let mut def = test_combatant(2, "meme_man", 1, 20, 10, vec![]);
        def.reflexes = 10;
        let base = hit_chance(&att, &def);
        def.applied_enemy_reflex_debuff = 3;
        let chilled = hit_chance(&att, &def);
        assert!(chilled > base);
    }
}
