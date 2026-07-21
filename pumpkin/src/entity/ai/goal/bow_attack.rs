use std::sync::Arc;

use rand::RngExt;

use pumpkin_data::{
    entity::EntityType,
    sound::{Sound, SoundCategory},
};
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase,
    ai::{
        goal::{Controls, Goal, GoalFuture},
        pathfinder::{NavigatorGoal, horizontal_distance_squared},
    },
    mob::Mob,
    projectile::arrow::{ArrowEntity, ArrowPickup},
};

/// A ground-based bow attack modeled after vanilla's skeleton attack loop.
///
/// It deliberately owns navigation and aiming, so ranged mobs do not fall back
/// to the generic melee goal while they have a valid target.
pub struct BowAttackGoal {
    speed: f64,
    attack_interval: i32,
    attack_radius_sq: f64,
    attack_cooldown: i32,
    update_path_countdown: i32,
    last_target_pos: Option<Vector3<f64>>,
    strafing_time: i32,
    strafe_clockwise: bool,
    strafe_backwards: bool,
}

impl BowAttackGoal {
    #[must_use]
    pub const fn new(speed: f64, attack_interval: i32, attack_radius: f64) -> Self {
        Self {
            speed,
            attack_interval,
            attack_radius_sq: attack_radius * attack_radius,
            attack_cooldown: 0,
            update_path_countdown: 0,
            last_target_pos: None,
            strafing_time: -1,
            strafe_clockwise: false,
            strafe_backwards: false,
        }
    }

    async fn shoot(&self, mob: &dyn Mob, target: &Arc<dyn EntityBase>) {
        let shooter = mob.get_entity();
        let target_entity = target.get_entity();
        let world = shooter.world.load_full();

        let arrow_entity = Entity::new(world.clone(), shooter.pos.load(), &EntityType::ARROW);
        let arrow = ArrowEntity::new_shot(arrow_entity, shooter, ArrowPickup::CreativeOnly);

        let arrow_pos = arrow.entity.pos.load();
        let target_pos = target_entity.pos.load();
        let x = target_pos.x - arrow_pos.x;
        let z = target_pos.z - arrow_pos.z;
        let horizontal_distance = (x * x + z * z).sqrt();
        let target_y = target_pos.y + f64::from(target_entity.height()) / 3.0;
        let y = target_y - arrow_pos.y + horizontal_distance * 0.2;

        // Vanilla's AbstractSkeleton uses power 1.6 and a difficulty-scaled
        // inaccuracy. Difficulty scaling is not wired here yet, so use the
        // normal-difficulty value (10 degrees).
        arrow.set_velocity(x, y, z, 1.6, 10.0);

        let arrow: Arc<dyn EntityBase> = Arc::new(arrow);
        world.spawn_entity(arrow).await;
        world.play_sound_fine(
            Sound::EntitySkeletonShoot,
            SoundCategory::Hostile,
            &shooter.pos.load(),
            1.0,
            1.0,
        );
    }
}

impl Goal for BowAttackGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            mob.get_mob_entity()
                .target
                .lock()
                .await
                .as_ref()
                .is_some_and(|target| target.get_entity().is_alive())
        })
    }

    fn should_continue<'a>(&'a self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let target = mob.get_mob_entity().target.lock().await.clone();
            let Some(target) = target else {
                return false;
            };

            if !target.get_entity().is_alive() {
                return false;
            }

            !target
                .get_player()
                .is_some_and(|player| player.is_spectator() || player.is_creative())
        })
    }

    fn start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.attack_cooldown = 0;
            self.update_path_countdown = 0;
            self.strafing_time = -1;
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            mob.get_mob_entity().navigator.lock().unwrap().stop();
            self.last_target_pos = None;
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let target = mob.get_mob_entity().target.lock().await.clone();
            let Some(target) = target else {
                return;
            };

            let mob_entity = mob.get_mob_entity();
            let mob_pos = mob.get_entity().pos.load();
            let target_pos = target.get_entity().pos.load();
            let distance_sq = mob_pos.squared_distance_to_vec(&target_pos);

            mob_entity
                .look_control
                .lock()
                .unwrap()
                .look_at_entity_with_range(&target, 30.0, 30.0);

            self.update_path_countdown = (self.update_path_countdown - 1).max(0);
            if distance_sq > self.attack_radius_sq {
                self.strafing_time = -1;
                let target_moved = self.last_target_pos.is_none_or(|last_pos| {
                    horizontal_distance_squared(target_pos, last_pos) >= 1.0
                });
                if self.update_path_countdown == 0 || target_moved {
                    mob_entity
                        .navigator
                        .lock()
                        .unwrap()
                        .set_progress(NavigatorGoal::new(mob_pos, target_pos, self.speed));
                    self.last_target_pos = Some(target_pos);
                    self.update_path_countdown = 10;
                }
            } else {
                mob_entity.navigator.lock().unwrap().stop();

                // Once in bow range, circle the target while keeping a
                // comfortable distance instead of becoming stationary.
                self.strafing_time += 1;
                if self.strafing_time >= 20 {
                    self.strafe_clockwise = mob.get_random().random_bool(0.5);
                    self.strafe_backwards = mob.get_random().random_bool(0.5);
                    self.strafing_time = 0;
                }

                if distance_sq < self.attack_radius_sq * 0.25 {
                    self.strafe_backwards = true;
                } else if distance_sq > self.attack_radius_sq * 0.75 {
                    self.strafe_backwards = false;
                }

                mob_entity.move_control.lock().unwrap().strafe(
                    if self.strafe_backwards { -0.5 } else { 0.5 },
                    if self.strafe_clockwise { 0.5 } else { -0.5 },
                );
            }

            self.attack_cooldown = (self.attack_cooldown - 1).max(0);
            if distance_sq <= self.attack_radius_sq && self.attack_cooldown == 0 {
                self.shoot(mob, &target).await;
                self.attack_cooldown = self.attack_interval;
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}
