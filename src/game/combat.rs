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
    pub hat_sprite: Option<String>,
    pub left_hand_sprite: Option<String>,
    pub right_hand_sprite: Option<String>,
    pub max_hp: i32,
    pub hp: i32,
    pub might: i32,
    pub reflexes: i32,
    pub wisdom: i32,
    pub properties: Vec<Property>,
    pub frozen_turns: i32,
    pub side: u8, // 0 or 1
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
    },
    Heal { healer: u32, target: u32, amount: i32 },
    Freeze { target: u32, sprite: String },
    Death { uid: u32, side: u8 },
    Summon { side: u8, combatant: Combatant },
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
        for (slot_id, sprite_out) in [
            (&m.hat, &mut hat_sprite),
            (&m.left_hand, &mut left_hand_sprite),
            (&m.right_hand, &mut right_hand_sprite),
        ] {
            if let Some(iid) = slot_id {
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
        let uid = *uid_start; *uid_start += 1;
        Some(Combatant {
            uid,
            def_id: def.id.clone(),
            sprite: def.sprite.clone(),
            hat_sprite, left_hand_sprite, right_hand_sprite,
            max_hp: hp, hp,
            might, reflexes, wisdom,
            properties: props,
            frozen_turns: 0,
            side,
        })
    }).collect()
}

fn hit_chance(att: &Combatant, def: &Combatant) -> f32 {
    // Simple model: base 0.7, modified by reflex diff. Clamped.
    let diff = (att.reflexes - def.reflexes) as f32;
    (0.7 + diff * 0.03).clamp(0.1, 0.95)
}

fn first_alive_idx(team: &[Combatant]) -> Option<usize> {
    team.iter().position(|c| c.hp > 0)
}

fn first_damaged_idx(team: &[Combatant], excluding: u32) -> Option<usize> {
    team.iter().enumerate()
        .filter(|(_, c)| c.hp > 0 && c.hp < c.max_hp && c.uid != excluding)
        .min_by_key(|(_, c)| c.max_hp - c.hp) // any damaged; pick least-damaged so heals top up
        .map(|(i, _)| i)
}

pub fn resolve_battle(left_build: &Build, right_build: &Build) -> BattleResult {
    let mut rng = rand::thread_rng();
    let mut uid_counter: u32 = 1;
    let mut left = build_team(left_build, 0, &mut uid_counter);
    let mut right = build_team(right_build, 1, &mut uid_counter);
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
            // SAFETY: actors/foes point into disjoint vecs (left & right).
            let actors = unsafe { &mut *actors };
            let foes = unsafe { &mut *foes };

            // Iterate by index since we mutate alongside.
            let actor_count = actors.len();
            for i in 0..actor_count {
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
                        actors[target_idx].hp = (actors[target_idx].hp + amount).min(actors[target_idx].max_hp);
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
                let foe_front = match first_alive_idx(foes) { Some(x) => x, None => break };
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

                let chance = hit_chance(&actors[i], &foes[foe_front]);
                let hit = rng.gen::<f32>() < chance;
                events.push(CombatEvent::Attack {
                    attacker: actors[i].uid,
                    target: foes[foe_front].uid,
                    ranged, projectile: projectile.clone(),
                    damage: if hit { damage_stat } else { 0 },
                    hit,
                });
                if hit {
                    foes[foe_front].hp -= damage_stat;
                    events.push(CombatEvent::Hp { uid: foes[foe_front].uid, hp: foes[foe_front].hp.max(0) });

                    // Freeze on hit
                    if let Some(spr) = actors[i].properties.iter().find_map(|p| {
                        if let Property::FreezeOnHit { sprite } = p { Some(sprite.clone()) } else { None }
                    }) {
                        if foes[foe_front].hp > 0 {
                            foes[foe_front].frozen_turns = 1;
                            events.push(CombatEvent::Freeze { target: foes[foe_front].uid, sprite: spr });
                        }
                    }

                    if foes[foe_front].hp <= 0 {
                        let dead_uid = foes[foe_front].uid;
                        let dead_def = foes[foe_front].def_id.clone();
                        let dead_side = foes[foe_front].side;
                        events.push(CombatEvent::Death { uid: dead_uid, side: dead_side });

                        // Trigger SummonOnEnemyDeath on the *attacker's* team.
                        let mut summons: Vec<Combatant> = vec![];
                        for ally in actors.iter() {
                            if ally.hp <= 0 { continue; }
                            for p in &ally.properties {
                                if let Property::SummonOnEnemyDeath { species } = p {
                                    if dead_def != *species && actors.iter().filter(|c| c.hp > 0).count() + summons.len() < MAX_TEAM {
                                        if let Some(def) = character_def(species) {
                                            let uid = uid_counter; uid_counter += 1;
                                            summons.push(Combatant {
                                                uid,
                                                def_id: def.id.clone(),
                                                sprite: def.sprite.clone(),
                                                hat_sprite: None, left_hand_sprite: None, right_hand_sprite: None,
                                                max_hp: def.hp, hp: def.hp,
                                                might: def.might, reflexes: def.reflexes, wisdom: def.wisdom,
                                                properties: def.properties.clone(),
                                                frozen_turns: 0,
                                                side,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                        for s in summons {
                            events.push(CombatEvent::Summon { side, combatant: s.clone() });
                            actors.push(s);
                        }
                    }
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
