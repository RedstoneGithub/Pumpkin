use std::sync::atomic::Ordering;

use crate::entity::EntityBase;
use pumpkin_data::{
    attributes::Attributes,
    particle::Particle,
    sound::{Sound, SoundCategory},
};
use pumpkin_util::math::vector3::Vector3;

use crate::{
    entity::{Entity, player::Player},
    world::World,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackType {
    Knockback,
    Critical,
    Sweeping,
    Strong,
    Weak,
    MaceSmash,
}

impl AttackType {
    pub async fn new(player: &Player, attack_cooldown_progress: f32) -> Self {
        let entity = &player.get_entity();

        let sprinting = entity.is_sprinting();
        let on_ground = entity.on_ground.load(Ordering::Relaxed);
        let fall_distance = player.living_entity.fall_distance.load();
        let held_item = player.inventory().held_item();
        let is_mace = {
            let stack = held_item.lock().await;
            stack.item.id == pumpkin_data::item::Item::MACE.id
        };

        if is_mace && !on_ground && fall_distance > 1.5 {
            return Self::MaceSmash;
        }

        let sword = {
            let stack = held_item.lock().await;
            stack.is_sword()
        };

        let is_strong = attack_cooldown_progress > 0.9;
        if sprinting && is_strong {
            return Self::Knockback;
        }

        if is_strong && !on_ground && fall_distance > 0.0 {
            return Self::Critical;
        }

        if sword && is_strong {
            return Self::Sweeping;
        }

        if is_strong { Self::Strong } else { Self::Weak }
    }
}

fn apply_knockback_from_source(
    victim: &dyn EntityBase,
    strength: f64,
    source_x: f64,
    source_z: f64,
) {
    // `LivingEntity#knockback` applies this attribute before delegating to
    // `Entity#knockback`.  Callers used to pass the bare Entity here, which
    // bypassed the resistance entirely.
    let resistance = victim.get_living_entity().map_or(0.0, |living| {
        living
            .get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE)
            .clamp(0.0, 1.0)
    });
    let strength = strength * (1.0 - resistance);
    if strength <= 0.0 {
        return;
    }

    let victim_entity = victim.get_entity();
    victim_entity.apply_knockback(strength * 0.5, source_x, source_z);
    victim_entity.send_velocity();
}

pub fn handle_knockback(attacker: &Entity, victim: &dyn EntityBase, strength: f64) {
    let yaw = attacker.yaw.load();
    apply_knockback_from_source(
        victim,
        strength,
        f64::from((yaw.to_radians()).sin()),
        f64::from(-(yaw.to_radians()).cos()),
    );

    let velocity = attacker.velocity.load();
    attacker.velocity.store(velocity.multiply(0.6, 1.0, 0.6));
}

/// Applies projectile knockback along its flight direction.
pub fn handle_projectile_knockback(
    victim: &dyn EntityBase,
    strength: f64,
    projectile_velocity: Vector3<f64>,
) {
    // `apply_knockback` expects a vector from the victim toward the source;
    // the projectile travels in the opposite direction.
    apply_knockback_from_source(
        victim,
        strength,
        -projectile_velocity.x,
        -projectile_velocity.z,
    );
}

pub fn spawn_sweep_particle(attacker_entity: &Entity, world: &World, pos: &Vector3<f64>) {
    let yaw = attacker_entity.yaw.load();
    let d = -f64::from((yaw.to_radians()).sin());
    let e = f64::from((yaw.to_radians()).cos());

    let scale = 0.5;
    let body_y = f64::from(attacker_entity.height()).mul_add(scale, pos.y);

    world.spawn_particle(
        Vector3::new(pos.x + d, body_y, pos.z + e),
        Vector3::new(0.0, 0.0, 0.0),
        0.0,
        0,
        Particle::SweepAttack,
    );
}

pub async fn player_attack_sound(pos: &Vector3<f64>, world: &World, attack_type: AttackType) {
    match attack_type {
        AttackType::Knockback => {
            world.play_sound(
                Sound::EntityPlayerAttackKnockback,
                SoundCategory::Players,
                pos,
            );
        }
        AttackType::Critical => {
            world.play_sound(Sound::EntityPlayerAttackCrit, SoundCategory::Players, pos);
        }
        AttackType::Sweeping => {
            world.play_sound(Sound::EntityPlayerAttackSweep, SoundCategory::Players, pos);
        }
        AttackType::Strong => {
            world.play_sound(Sound::EntityPlayerAttackStrong, SoundCategory::Players, pos);
        }
        AttackType::Weak => {
            world.play_sound(Sound::EntityPlayerAttackWeak, SoundCategory::Players, pos);
        }
        AttackType::MaceSmash => {
            world.play_sound(Sound::ItemMaceSmashAir, SoundCategory::Players, pos);
        }
    }
}
