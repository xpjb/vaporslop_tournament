use crate::game::data::*;
use crate::game::types::*;
use rand::Rng;
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
    pub hat_sprite: Option<String>,
    pub left_hand_sprite: Option<String>,
    pub right_hand_sprite: Option<String>,
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
    /// From gear (`ReviveOnce`); decremented when a revival triggers.
    #[serde(default)]
    pub revive_charges: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CombatEvent {
    Start { left: Vec<Combatant>, right: Vec<Combatant> },
    Attack {
        attacker: u32,
        target: u32,
        ranged: bool,
        projectile: Option<String>,
        damage: i32,
        hit: bool,
        #[serde(default)]
        critical: bool,
    },
    Heal { healer: u32, target: u32, amount: i32 },
    Freeze { target: u32, sprite: String },
    Death { uid: u32, side: u8 },
    Revive { uid: u32, hp: i32 },
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
    },
    Hp { uid: u32, hp: i32 },
    End { winner: Option<u8> }, // None = draw
}

pub struct BattleResult {
    pub events: Vec<CombatEvent>,
    pub winner: Option<u8>, // None = draw, Some(0) left, Some(1) right
}

fn build_team(build: &Build, side: u8, uid_start: &mut u32) -> Vec<Combatant> {
    build.team.iter().filter_map(|m| {
        let def = character_def(&m.def_id)?;
        let mut might = def.might;
        let mut reflexes = def.reflexes;
        let mut wisdom = def.wisdom;
        let mut hp = def.hp;
        let mut props = def.properties.clone();
        let mut hat_sprite = None;
        let mut left_hand_sprite = None;
        let mut right_hand_sprite = None;
        let mut hat_id = None;
        let mut left_hand_id = None;
        let mut right_hand_id = None;
        for (slot_id, sprite_out, id_out) in [
            (&m.hat, &mut hat_sprite, &mut hat_id),
            (&m.left_hand, &mut left_hand_sprite, &mut left_hand_id),
            (&m.right_hand, &mut right_hand_sprite, &mut right_hand_id),
        ] {
            if let Some(iid) = slot_id {
                *id_out = Some(iid.clone());
                if let Some(idef) = item_def(iid) {
                    *sprite_out = Some(idef.sprite.clone());
                    for p in &idef.properties {
                        if let Property::StatBonus { might: m_, reflexes: r_, wisdom: w_, hp: h_ } = p {
                            might += m_; reflexes += r_; wisdom += w_; hp += h_;
                        } else {
                            props.push(p.clone());
                        }
                    }
                }
            }
        }
        let revive_charges = props.iter().filter(|p| matches!(p, Property::ReviveOnce)).count() as u8;
        props.retain(|p| !matches!(p, Property::ReviveOnce));
        let uid = *uid_start; *uid_start += 1;
        Some(Combatant {
            uid,
            def_id: def.id.clone(),
            sprite: def.sprite.clone(),
            hat_id, left_hand_id, right_hand_id,
            hat_sprite, left_hand_sprite, right_hand_sprite,
            max_hp: hp, hp,
            might, reflexes, wisdom,
            properties: props,
            frozen_turns: 0,
            side,
            applied_front_might: 0,
            applied_front_reflexes: 0,
            applied_front_wisdom: 0,
            formation_hp_bonus: 0,
            revive_charges,
        })
    }).collect()
}

#[inline]
fn effective_max_hp(c: &Combatant) -> i32 {
    c.max_hp + c.formation_hp_bonus
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
        let eff_max = c.max_hp + c.formation_hp_bonus;
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
                });
            }
        }
    }
}

/// Hit chance vs the defender's frontliner. Same formula for melee and ranged.
fn hit_chance(att: &Combatant, def: &Combatant) -> f32 {
    let diff = (att.reflexes - def.reflexes) as f32;
    (0.7 + diff * 0.03).clamp(0.1, 0.95)
}

/// After a successful hit, roll crit from attacker's properties (first matching `CritStrike`).
fn roll_crit_damage(base: i32, props: &[Property], rng: &mut impl Rng) -> (i32, bool) {
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
    if rng.gen::<f32>() < p {
        (base.saturating_mul(2), true)
    } else {
        (base, false)
    }
}

fn first_alive_idx(team: &[Combatant]) -> Option<usize> {
    team.iter().position(|c| c.hp > 0)
}

fn first_damaged_idx(team: &[Combatant], excluding: u32) -> Option<usize> {
    team.iter().enumerate()
        .filter(|(_, c)| c.hp > 0 && c.hp < effective_max_hp(c) && c.uid != excluding)
        .min_by_key(|(_, c)| effective_max_hp(c) - c.hp) // any damaged; pick least-damaged so heals top up
        .map(|(i, _)| i)
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
        if ally.hp <= 0 { continue; }
        for p in &ally.properties {
            if let Property::SummonOnEnemyDeath { species } = p {
                if dead_def != species.as_str() && actors.iter().filter(|c| c.hp > 0).count() + summons.len() < MAX_TEAM {
                    if let Some(def) = character_def(species) {
                        let uid = *uid_counter;
                        *uid_counter += 1;
                        summons.push((
                            Combatant {
                                uid,
                                def_id: def.id.clone(),
                                sprite: def.sprite.clone(),
                                hat_id: None, left_hand_id: None, right_hand_id: None,
                                hat_sprite: None, left_hand_sprite: None, right_hand_sprite: None,
                                max_hp: def.hp, hp: def.hp,
                                might: def.might, reflexes: def.reflexes, wisdom: def.wisdom,
                                properties: def.properties.clone(),
                                frozen_turns: 0,
                                side: attacker_side,
                                applied_front_might: 0,
                                applied_front_reflexes: 0,
                                applied_front_wisdom: 0,
                                formation_hp_bonus: 0,
                                revive_charges: 0,
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
        if ally.hp <= 0 { continue; }
        for p in &ally.properties {
            if let Property::SummonOnAllyDeath { species } = p {
                if dead_def != species.as_str() && foes.iter().filter(|c| c.hp > 0).count() + ally_summons.len() < MAX_TEAM {
                    if let Some(def) = character_def(species) {
                        let uid = *uid_counter;
                        *uid_counter += 1;
                        ally_summons.push((
                            Combatant {
                                uid,
                                def_id: def.id.clone(),
                                sprite: def.sprite.clone(),
                                hat_id: None, left_hand_id: None, right_hand_id: None,
                                hat_sprite: None, left_hand_sprite: None, right_hand_sprite: None,
                                max_hp: def.hp, hp: def.hp,
                                might: def.might, reflexes: def.reflexes, wisdom: def.wisdom,
                                properties: def.properties.clone(),
                                frozen_turns: 0,
                                side: dead_side,
                                applied_front_might: 0,
                                applied_front_reflexes: 0,
                                applied_front_wisdom: 0,
                                formation_hp_bonus: 0,
                                revive_charges: 0,
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

fn handle_foe_killed(
    foes: &mut Vec<Combatant>,
    actors: &mut Vec<Combatant>,
    foe_idx: usize,
    attacker_side: u8,
    events: &mut Vec<CombatEvent>,
    uid_counter: &mut u32,
) {
    let dead_uid = foes[foe_idx].uid;
    let dead_def = foes[foe_idx].def_id.clone();
    let dead_side = foes[foe_idx].side;
    events.push(CombatEvent::Death { uid: dead_uid, side: dead_side });
    apply_might_on_ally_death(foes, events);
    summon_on_enemy_death(actors, &dead_def, attacker_side, events, uid_counter);
    summon_on_ally_death(foes, &dead_def, dead_side, events, uid_counter);
}

/// Apply item effects that trigger when an ally dies (`foes` is that ally's team).
fn apply_might_on_ally_death(foes: &mut [Combatant], events: &mut Vec<CombatEvent>) {
    for c in foes.iter_mut() {
        if c.hp <= 0 {
            continue;
        }
        let gain: i32 = c
            .properties
            .iter()
            .filter_map(|p| {
                if let Property::MightOnAllyDeath { might } = p {
                    Some(*might)
                } else {
                    None
                }
            })
            .sum();
        if gain == 0 {
            continue;
        }
        c.might += gain;
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
        });
    }
}

pub fn resolve_battle(left_build: &Build, right_build: &Build) -> BattleResult {
    let mut rng = rand::thread_rng();
    let mut uid_counter: u32 = 1;
    let mut left = build_team(left_build, 0, &mut uid_counter);
    let mut right = build_team(right_build, 1, &mut uid_counter);
    refresh_formation_front_aura(&mut left, None);
    refresh_formation_front_aura(&mut right, None);
    let mut events = vec![CombatEvent::Start { left: left.clone(), right: right.clone() }];

    // Safety: cap turns to avoid infinite loops.
    for _turn in 0..200 {
        if left.iter().all(|c| c.hp <= 0) || right.iter().all(|c| c.hp <= 0) { break; }

        // Each side acts once per tick; collect actions for both, apply afterward.
        // But to keep things simple we alternate: left side then right side, killing in real-time.
        for side in 0..2u8 {
            let (actors, foes) = if side == 0 {
                (&mut left as *mut Vec<Combatant>, &mut right as *mut Vec<Combatant>)
            } else {
                (&mut right as *mut Vec<Combatant>, &mut left as *mut Vec<Combatant>)
            };
            // Iterate over the actors that existed at tick start; summons join the
            // formation immediately, but don't act until the next tick.
            let actor_uids: Vec<u32> = unsafe { (&*actors).iter().map(|c| c.uid).collect() };
            for actor_uid in actor_uids {
                let mut needs_aura = false;
                {
                    let actors = unsafe { &mut *actors };
                    let foes = unsafe { &mut *foes };

                    let Some(i) = actors.iter().position(|c| c.uid == actor_uid) else { continue; };
                    if actors[i].hp <= 0 { continue; }
                    if actors[i].frozen_turns > 0 {
                        actors[i].frozen_turns -= 1;
                        continue;
                    }
                    let is_front = first_alive_idx(actors) == Some(i);
                    let has_ranged = actors[i].properties.iter().any(|p| matches!(p, Property::Ranged { .. }));
                    let is_healer = actors[i].properties.iter().any(|p| matches!(p, Property::Healer));

                    // Healer: heal frontmost damaged friendly (not self) if any, else attack.
                    if is_healer {
                        if let Some(target_idx) = first_damaged_idx(actors, actors[i].uid) {
                            let amount = actors[i].wisdom;
                            let cap = effective_max_hp(&actors[target_idx]);
                            actors[target_idx].hp = (actors[target_idx].hp + amount).min(cap);
                            events.push(CombatEvent::Heal {
                                healer: actors[i].uid,
                                target: actors[target_idx].uid,
                                amount,
                            });
                            events.push(CombatEvent::Hp { uid: actors[target_idx].uid, hp: actors[target_idx].hp });
                            continue;
                        }
                    }

                    // Attack: front=melee, non-front+ranged=ranged, otherwise no action.
                    if first_alive_idx(foes).is_none() { break; }
                    let (ranged, projectile, damage_stat) = if is_front {
                        (false, None, actors[i].might)
                    } else if has_ranged {
                        let proj = actors[i].properties.iter().find_map(|p| {
                            if let Property::Ranged { projectile } = p { Some(projectile.clone()) } else { None }
                        });
                        (true, proj, actors[i].wisdom)
                    } else {
                        continue;
                    };

                    let cleave_count = if !ranged {
                        actors[i].properties.iter().find_map(|p| {
                            if let Property::MeleeCleave { count } = p {
                                Some((*count).max(1) as usize)
                            } else {
                                None
                            }
                        }).unwrap_or(1)
                    } else {
                        1
                    };

                    let foe_indices: Vec<usize> = foes.iter().enumerate()
                        .filter(|(_, c)| c.hp > 0)
                        .take(cleave_count)
                        .map(|(idx, _)| idx)
                        .collect();

                    for &foe_idx in &foe_indices {
                        let chance = hit_chance(&actors[i], &foes[foe_idx]);
                        let hit = rng.gen::<f32>() < chance;
                        let (damage, critical) = if hit {
                            roll_crit_damage(damage_stat, &actors[i].properties, &mut rng)
                        } else {
                            (0, false)
                        };
                        events.push(CombatEvent::Attack {
                            attacker: actors[i].uid,
                            target: foes[foe_idx].uid,
                            ranged,
                            projectile: projectile.clone(),
                            damage,
                            hit,
                            critical,
                        });
                        if hit {
                            foes[foe_idx].hp -= damage;
                            let mut revived = false;
                            if foes[foe_idx].hp <= 0 && foes[foe_idx].revive_charges > 0 {
                                foes[foe_idx].revive_charges -= 1;
                                foes[foe_idx].hp = effective_max_hp(&foes[foe_idx]);
                                events.push(CombatEvent::Revive {
                                    uid: foes[foe_idx].uid,
                                    hp: foes[foe_idx].hp,
                                });
                                events.push(CombatEvent::Hp {
                                    uid: foes[foe_idx].uid,
                                    hp: foes[foe_idx].hp,
                                });
                                revived = true;
                            } else {
                                events.push(CombatEvent::Hp {
                                    uid: foes[foe_idx].uid,
                                    hp: foes[foe_idx].hp.max(0),
                                });
                            }

                            if let Some(spr) = actors[i].properties.iter().find_map(|p| {
                                if let Property::FreezeOnHit { sprite } = p { Some(sprite.clone()) } else { None }
                            }) {
                                if foes[foe_idx].hp > 0 {
                                    foes[foe_idx].frozen_turns = 1;
                                    events.push(CombatEvent::Freeze { target: foes[foe_idx].uid, sprite: spr });
                                }
                            }

                            if !revived && foes[foe_idx].hp <= 0 {
                                handle_foe_killed(foes, actors, foe_idx, side, &mut events, &mut uid_counter);
                                needs_aura = true;
                            }
                        }
                    }
                }
                if needs_aura {
                    refresh_formation_front_aura(&mut left, Some(&mut events));
                    refresh_formation_front_aura(&mut right, Some(&mut events));
                }
            }
        }
    }

    let left_alive = left.iter().any(|c| c.hp > 0);
    let right_alive = right.iter().any(|c| c.hp > 0);
    let winner = match (left_alive, right_alive) {
        (true, false) => Some(0),
        (false, true) => Some(1),
        _ => None,
    };
    events.push(CombatEvent::End { winner });
    BattleResult { events, winner }
}
