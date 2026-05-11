use crate::game::data::*;
use crate::game::rng::Rng;
use crate::game::types::*;

pub fn roll_shop(rng: &mut Rng) -> Shop {
    let chars: Vec<String> = character_defs().iter().map(|c| c.id.clone()).collect();
    let items: Vec<String> = item_defs().iter().map(|i| i.id.clone()).collect();
    let mut shop = Shop::default();
    for _ in 0..SHOP_CHAR_SLOTS {
        shop.characters.push(rng.choice(&chars).cloned());
    }
    for _ in 0..SHOP_ITEM_SLOTS {
        shop.items.push(rng.choice(&items).cloned());
    }
    shop
}

/// Generate an AI build for a target gold budget using light combat heuristics.
pub fn ai_ladder_build(target_cost: i32, rng: &mut Rng) -> Build {
    let mut spent = 0;
    let mut team: Vec<TeamMember> = vec![];
    let chars: Vec<&CharacterDef> = character_defs().iter().collect();

    while team.len() < MAX_TEAM && spent < target_cost {
        let remaining = target_cost - spent;
        let affordable: Vec<&CharacterDef> = chars
            .iter()
            .copied()
            .filter(|c| c.cost <= remaining)
            .collect();
        if affordable.is_empty() {
            break;
        }

        let needs_front = !team.iter().any(is_frontline_member);
        let prefer_backline = !needs_front && rng.chance(0.65);
        let mut pool: Vec<&CharacterDef> = affordable
            .iter()
            .copied()
            .filter(|c| {
                if needs_front {
                    is_frontline_def(c)
                } else if prefer_backline {
                    is_backline_def(c)
                } else {
                    is_frontline_def(c)
                }
            })
            .collect();
        if pool.is_empty() {
            pool = affordable;
        }

        pool.sort_by_key(|c| {
            if is_frontline_def(c) {
                front_score(c)
            } else {
                back_score(c)
            }
        });
        pool.reverse();
        let pick_pool_len = pool.len().min(3);
        let pick = rng.choice(&pool[..pick_pool_len]).copied().unwrap();
        spent += pick.cost;
        team.push(TeamMember {
            def_id: pick.id.clone(),
            hat: None,
            left_hand: None,
            right_hand: None,
            hand_3: None,
            hand_4: None,
        });
    }

    arrange_ai_team(&mut team);
    equip_ai_items(&mut team, target_cost - spent, rng);
    arrange_ai_team(&mut team);

    Build { team }
}

fn equip_ai_items(team: &mut [TeamMember], mut remaining: i32, rng: &mut Rng) {
    if remaining <= 0 {
        return;
    }

    loop {
        let mut candidates: Vec<(usize, &ItemDef, i32)> = vec![];
        for (member_idx, member) in team.iter().enumerate() {
            let Some(def) = character_def(&member.def_id) else {
                continue;
            };
            let wants_wisdom = is_backline_def(def);
            for item in item_defs().iter() {
                if item.cost <= remaining && can_equip_item(member, item) {
                    candidates.push((member_idx, item, item_fit_score(item, wants_wisdom)));
                }
            }
        }

        if candidates.is_empty() {
            break;
        }

        candidates.sort_by_key(|(_, item, score)| (*score, item.cost));
        candidates.reverse();
        let pick_pool_len = candidates.len().min(3);
        let (member_idx, item, _) = rng.choice(&candidates[..pick_pool_len]).copied().unwrap();
        equip_item(&mut team[member_idx], item);
        remaining -= item.cost;
    }
}

fn can_equip_item(member: &TeamMember, item: &ItemDef) -> bool {
    match item.slot {
        GearSlot::Hat => member.hat.is_none(),
        GearSlot::Hand => member_hand_slot_count(member) > member_filled_hands(member),
    }
}

fn equip_item(member: &mut TeamMember, item: &ItemDef) {
    match item.slot {
        GearSlot::Hat => member.hat = Some(item.id.clone()),
        GearSlot::Hand => {
            let limit = member_hand_slot_count(member);
            for slot in ItemSlot::HAND_SLOTS.iter().take(limit as usize) {
                let dest = member.hand_slot_mut(*slot);
                if dest.is_none() {
                    *dest = Some(item.id.clone());
                    return;
                }
            }
        }
    }
}

fn member_hand_slot_count(member: &TeamMember) -> u8 {
    character_def(&member.def_id)
        .map(|d| d.hand_slots())
        .unwrap_or(2)
}

fn member_filled_hands(member: &TeamMember) -> u8 {
    ItemSlot::HAND_SLOTS
        .iter()
        .filter(|slot| member.hand_slot(**slot).is_some())
        .count() as u8
}

fn arrange_ai_team(team: &mut [TeamMember]) {
    team.sort_by_key(|m| {
        let Some(def) = character_def(&m.def_id) else {
            return 0;
        };
        if is_frontline_def(def) {
            10_000 + front_score(def)
        } else {
            back_score(def)
        }
    });
    team.reverse();
}

fn is_frontline_member(member: &TeamMember) -> bool {
    character_def(&member.def_id)
        .map(is_frontline_def)
        .unwrap_or(false)
}

fn is_frontline_def(def: &CharacterDef) -> bool {
    !is_backline_def(def)
}

fn is_backline_def(def: &CharacterDef) -> bool {
    def.properties.iter().any(|p| {
        matches!(
            p,
            Property::Ranged { .. } | Property::Healer | Property::BuffFormationFront { .. }
        )
    })
}

fn front_score(def: &CharacterDef) -> i32 {
    def.hp * 3 + def.might * 8 + def.reflexes * 2
}

fn back_score(def: &CharacterDef) -> i32 {
    def.wisdom * 8 + def.reflexes * 2 + def.hp
}

fn item_fit_score(item: &ItemDef, wants_wisdom: bool) -> i32 {
    item.properties
        .iter()
        .map(|p| match p {
            Property::StatBonus {
                might,
                reflexes,
                wisdom,
                hp,
            } => {
                if wants_wisdom {
                    wisdom * 8 + reflexes * 2 + hp + might
                } else {
                    hp * 3 + might * 8 + reflexes * 2 + wisdom
                }
            }
            Property::Armour { value } => *value * 3,
            _ => 10,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_ladder_build_puts_backline_behind_frontline() {
        for i in 0..25 {
            let mut rng = Rng::new(0xA1B2_C3D4 ^ i as u32);
            let build = ai_ladder_build(700, &mut rng);
            assert!(!build.team.is_empty());

            let first_backline = build.team.iter().position(|m| {
                character_def(&m.def_id)
                    .map(is_backline_def)
                    .unwrap_or(false)
            });
            let last_frontline = build.team.iter().rposition(is_frontline_member);

            if let (Some(first_backline), Some(last_frontline)) = (first_backline, last_frontline) {
                assert!(last_frontline < first_backline);
            }
        }
    }
}
