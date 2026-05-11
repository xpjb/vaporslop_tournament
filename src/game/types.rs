use serde::{Deserialize, Serialize};

pub const MAX_TEAM: usize = 8;
pub const SHOP_CHAR_SLOTS: usize = 5;
pub const SHOP_ITEM_SLOTS: usize = 5;
pub const REROLL_COST: i32 = 10;
pub const STARTING_MONEY: i32 = 100;
pub const WIN_REWARD: i32 = 100;
pub const LOSE_REWARD: i32 = 50;
pub const MAX_LOSSES: i32 = 3;
pub const MAX_WINS: i32 = 30;
pub const STARTING_MMR: i32 = 1000;
pub const MMR_K_FACTOR: f64 = 32.0;
pub const SELL_RATIO: f32 = 1.0; // 100%

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Property {
    Ranged {
        projectile: String,
    },
    Healer,
    FreezeOnHit {
        sprite: String,
    },
    SummonOnEnemyDeath {
        species: String,
    },
    SummonOnAllyDeath {
        species: String,
    },
    /// When this unit dies, damage the opposing front unit by `might_multiplier * might`.
    DamageEnemyOnDeath {
        might_multiplier: i32,
    },
    /// When this unit dies, living allies gain `amount` to all stats.
    TeamStatsOnDeath {
        amount: i32,
    },
    /// Living wearer gains this much might each time an ally on the same side dies.
    MightOnAllyDeath {
        might: i32,
    },
    /// Living wearer gains these stats (and `hp` to max HP and current HP, capped) each time an ally on the same side dies.
    StatsOnAllyDeath {
        might: i32,
        reflexes: i32,
        wisdom: i32,
        hp: i32,
    },
    /// Living wearer gains these stats (and `hp` to max HP and current HP, capped) each time they kill an enemy.
    StatsOnKill {
        might: i32,
        reflexes: i32,
        wisdom: i32,
        hp: i32,
    },
    /// On each damaging hit, chance_percent roll (0–100) to deal double damage.
    CritStrike {
        chance_percent: u8,
    },
    /// Once per battle, when HP reaches 0, revive at full effective HP (charges tracked at runtime).
    ReviveOnce,
    /// Once per battle, when HP reaches 0, revive at full HP and reposition to the back of the formation.
    ReviveAtBackOnce,
    /// While alive, wearer gains `amount` to all stats per *other* living ally on their side.
    StatsPerLivingAlly {
        amount: i32,
    },
    /// Melee only: hit the first `count` living enemies in formation order per swing.
    MeleeCleave {
        count: u8,
    },
    /// Melee only: add `plus` extra cleave targets (stacks across gear). Combined with innate `MeleeCleave` max or 1.
    MeleeCleaveBonus {
        plus: u8,
    },
    /// May melee (might-based) from the second living formation slot; overrides `Ranged` there.
    MeleeFromSecond,
    StatBonus {
        might: i32,
        reflexes: i32,
        wisdom: i32,
        hp: i32,
    },
    /// Stacks. Total armour subtracts that much from each hit’s damage (min 0).
    Armour {
        value: i32,
    },
    /// While alive, adds these stats to the ally in formation front (first living slot).
    BuffFormationFront {
        might: i32,
        reflexes: i32,
        wisdom: i32,
        hp: i32,
    },
    /// While alive, each living enemy loses this much reflexes (hit-chance defense; stacks across allies).
    DebuffEnemyReflexes {
        amount: i32,
    },
    /// On each landed hit, foe loses `amount` from might, reflexes, wisdom, and max HP (and current HP capped); totals stack across gear.
    DrainEnemyStatsOnHit {
        amount: i32,
    },
}

/// Where an item may be equipped. Hand items go into any free hand slot.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GearSlot {
    Hat,
    Hand,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ItemSlot {
    Hat,
    LeftHand,
    RightHand,
    Hand3,
    Hand4,
}

impl ItemSlot {
    pub const HAND_SLOTS: [ItemSlot; 4] = [
        ItemSlot::LeftHand,
        ItemSlot::RightHand,
        ItemSlot::Hand3,
        ItemSlot::Hand4,
    ];
}

fn default_hand_slots() -> u8 {
    2
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
    #[serde(default = "default_hand_slots")]
    pub hand_slots: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemDef {
    pub id: String,
    pub name: String,
    pub sprite: String,
    pub cost: i32,
    pub slot: GearSlot,
    pub properties: Vec<Property>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerProfile {
    pub player_id: String,
    pub name: String,
    pub selected_avatar: String,
    pub best_wins: i32,
    pub ultimate_victories: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileAvatarDef {
    pub id: String,
    pub name: String,
    pub sprite: String,
    pub required_wins: i32,
    pub required_ultimate_victories: i32,
}

/// A character placed on a team — references defs and equipped item ids.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    pub def_id: String,
    pub hat: Option<String>,        // item id
    pub left_hand: Option<String>,  // item id
    pub right_hand: Option<String>, // item id
    #[serde(default)]
    pub hand_3: Option<String>,
    #[serde(default)]
    pub hand_4: Option<String>,
}

impl TeamMember {
    pub fn item_ids(&self) -> impl Iterator<Item = &String> {
        self.hat
            .iter()
            .chain(self.left_hand.iter())
            .chain(self.right_hand.iter())
            .chain(self.hand_3.iter())
            .chain(self.hand_4.iter())
    }

    pub fn hand_slot(&self, slot: ItemSlot) -> &Option<String> {
        match slot {
            ItemSlot::LeftHand => &self.left_hand,
            ItemSlot::RightHand => &self.right_hand,
            ItemSlot::Hand3 => &self.hand_3,
            ItemSlot::Hand4 => &self.hand_4,
            ItemSlot::Hat => &self.hat,
        }
    }

    pub fn hand_slot_mut(&mut self, slot: ItemSlot) -> &mut Option<String> {
        match slot {
            ItemSlot::LeftHand => &mut self.left_hand,
            ItemSlot::RightHand => &mut self.right_hand,
            ItemSlot::Hand3 => &mut self.hand_3,
            ItemSlot::Hand4 => &mut self.hand_4,
            ItemSlot::Hat => &mut self.hat,
        }
    }
}

/// What the player owns / how their build looks. Persisted as JSON.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Build {
    pub team: Vec<TeamMember>, // ordered, max 5
}

impl Build {
    pub fn cost_value(&self) -> i32 {
        self.team
            .iter()
            .map(|m| {
                let mut c = crate::game::data::character_def(&m.def_id)
                    .map(|d| d.cost)
                    .unwrap_or(0);
                for iid in m.item_ids() {
                    c += crate::game::data::item_def(iid)
                        .map(|d| d.cost)
                        .unwrap_or(0);
                }
                c
            })
            .sum()
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
    pub player_id: String,
    pub name: String,
    pub money: i32,
    pub wins: i32,
    pub losses: i32,
    pub streak: i32,
    pub best_streak: i32,
    pub mmr: i32,
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
