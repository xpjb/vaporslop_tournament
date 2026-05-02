use crate::game::data::*;
use crate::game::types::*;
use rand::seq::SliceRandom;
use rand::Rng;

pub fn roll_shop() -> Shop {
    let mut rng = rand::thread_rng();
    let chars: Vec<String> = character_defs().iter().map(|c| c.id.clone()).collect();
    let items: Vec<String> = item_defs().iter().map(|i| i.id.clone()).collect();
    let mut shop = Shop::default();
    for _ in 0..SHOP_CHAR_SLOTS {
        shop.characters.push(Some(chars.choose(&mut rng).cloned().unwrap()));
    }
    for _ in 0..SHOP_ITEM_SLOTS {
        shop.items.push(Some(items.choose(&mut rng).cloned().unwrap()));
    }
    shop
}

/// Generate a random build at roughly the given target cost (best-effort).
pub fn random_build(target_cost: i32) -> Build {
    let mut rng = rand::thread_rng();
    let mut spent = 0;
    let mut team: Vec<TeamMember> = vec![];
    let chars: Vec<&CharacterDef> = character_defs().iter().collect();
    let items_hat: Vec<&ItemDef> = item_defs().iter().filter(|i| i.slot == ItemSlot::Hat).collect();
    let items_lh: Vec<&ItemDef> = item_defs().iter().filter(|i| i.slot == ItemSlot::LeftHand).collect();
    let items_rh: Vec<&ItemDef> = item_defs().iter().filter(|i| i.slot == ItemSlot::RightHand).collect();

    while team.len() < MAX_TEAM && spent < target_cost {
        let remaining = target_cost - spent;
        let affordable: Vec<&&CharacterDef> = chars.iter().filter(|c| c.cost <= remaining + 30).collect();
        if affordable.is_empty() { break; }
        let pick = affordable.choose(&mut rng).unwrap();
        spent += pick.cost;
        let mut m = TeamMember { def_id: pick.id.clone(), hat: None, left_hand: None, right_hand: None };
        if rng.gen_bool(0.4) {
            if let Some(it) = items_hat.choose(&mut rng) { m.hat = Some(it.id.clone()); spent += it.cost; }
        }
        if rng.gen_bool(0.3) {
            if let Some(it) = items_lh.choose(&mut rng) { m.left_hand = Some(it.id.clone()); spent += it.cost; }
        }
        if rng.gen_bool(0.3) {
            if let Some(it) = items_rh.choose(&mut rng) { m.right_hand = Some(it.id.clone()); spent += it.cost; }
        }
        team.push(m);
    }
    Build { team }
}
