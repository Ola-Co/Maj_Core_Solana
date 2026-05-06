use anchor_lang::prelude::*;

#[account]
pub struct MajRegistry {
    pub total_instances: u64,
    pub bump: u8,
}

impl MajRegistry {
    pub const SPACE: usize = 8 + 8 + 1; // 17
}
