use serde::{Deserialize, Serialize};

pub const MAX_TEAM: usize = 8;
pub const SHOP_CHAR_SLOTS: usize = 5;
pub const SHOP_ITEM_SLOTS: usize = 5;
pub const REROLL_COST: i32 = 10;
pub const STARTING_MONEY: i32 = 100;
pub const WIN_REWARD: i32 = 100;
pub const LOSE_REWARD: i32 = 50;
pub const MAX_LOSSES: i32 = 3;
pub const SELL_RATIO: f32 = 1.0; // 100%

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Property {
    Ranged { projectile: String },
    Healer,
    FreezeOnHit { sprite: String },
    SummonOnEnemyDeath { species: String },
    StatBonus { might: i32, reflexes: i32, wisdom: i32, hp: i32 },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ItemSlot {
    Hat,
    LeftHand,
    RightHand,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterDef {
    pub id: String,
    pub name: String,
    pub sprite: String,
    pub cost: i32,
    pub might: i32,
    pub reflexes: i32,
    pub wisdom: i32,
    pub hp: i32,
    pub properties: Vec<Property>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemDef {
    pub id: String,
    pub name: String,
    pub sprite: String,
    pub cost: i32,
    pub slot: ItemSlot,
    pub properties: Vec<Property>,
}

/// A character placed on a team — references defs and equipped item ids.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    pub def_id: String,
    pub hat: Option<String>,        // item id
    pub left_hand: Option<String>,  // item id
    pub right_hand: Option<String>, // item id
}

impl TeamMember {
    pub fn item_ids(&self) -> impl Iterator<Item = &String> {
        self.hat.iter().chain(self.left_hand.iter()).chain(self.right_hand.iter())
    }
}

/// What the player owns / how their build looks. Persisted as JSON.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Build {
    pub team: Vec<TeamMember>, // ordered, max 5
}

impl Build {
    pub fn cost_value(&self) -> i32 {
        self.team.iter().map(|m| {
            let mut c = crate::game::data::character_def(&m.def_id).map(|d| d.cost).unwrap_or(0);
            for iid in m.item_ids() {
                c += crate::game::data::item_def(iid).map(|d| d.cost).unwrap_or(0);
            }
            c
        }).sum()
    }
}

/// Shop offering for the current shop session.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Shop {
    pub characters: Vec<Option<String>>, // def_ids; None == bought
    pub items: Vec<Option<String>>,      // item ids; None == bought
}

/// In-DB run record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub name: String,
    pub money: i32,
    pub wins: i32,
    pub losses: i32,
    pub streak: i32,
    pub alive: bool,
    pub build: Build,
    pub shop: Shop,
    pub phase: Phase,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Shop,
    Battle,
    GameOver,
}
