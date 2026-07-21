use super::{Controls, Goal};
use crate::entity::EntityBase;
use crate::entity::ai::goal::GoalFuture;
use crate::entity::ai::pathfinder::{NavigatorGoal, horizontal_distance_squared};
use crate::entity::mob::Mob;
use crate::entity::predicate::EntityPredicate;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

const MAX_ATTACK_TIME: i64 = 20;
const AIRBORNE_TARGET_GROUND_SNAP_DISTANCE: i32 = 2;

pub struct MeleeAttackGoal {
    goal_control: Controls,
    speed: f64,
    pause_when_mob_idle: bool,
    //path: Path, TODO: add path when Navigation is implemented
    #[expect(dead_code)]
    target_location: Vector3<f64>,
    update_countdown_ticks: i32,
    pub cooldown: i32,
    #[expect(dead_code)]
    attack_interval_ticks: i32,
    last_update_time: i64,
    last_target_position: Option<Vector3<f64>>,
}

impl MeleeAttackGoal {
    #[must_use]
    pub fn new(speed: f64, pause_when_mob_idle: bool) -> Self {
        Self {
            goal_control: Controls::MOVE | Controls::LOOK,
            speed: speed.max(0.23), // Ensure minimum visible speed
            pause_when_mob_idle,
            target_location: Vector3::new(0.0, 0.0, 0.0),
            update_countdown_ticks: 0,
            cooldown: 0,
            attack_interval_ticks: 20,
            last_update_time: 0,
            last_target_position: None,
        }
    }

    #[must_use]
    pub fn get_max_cooldown(&self) -> i32 {
        self.get_tick_count(20)
    }

    /// Ground navigation cannot route to a target's airborne block position.
    /// For a short knockback/jump arc, pursue the solid ground directly below
    /// the target while still aiming and measuring attack reach at its real
    /// position.
    fn navigation_target_position(target: &dyn EntityBase) -> Vector3<f64> {
        let entity = target.get_entity();
        let target_pos = entity.pos.load();
        if entity.on_ground.load(std::sync::atomic::Ordering::Relaxed) {
            return target_pos;
        }

        let world = entity.world.load();
        let target_y = target_pos.y.floor() as i32;
        for y in ((target_y - AIRBORNE_TARGET_GROUND_SNAP_DISTANCE)..=target_y).rev() {
            let ground_pos =
                BlockPos::new(target_pos.x.floor() as i32, y, target_pos.z.floor() as i32);
            if world.get_block_state(&ground_pos).is_solid_block() {
                let ground_y = f64::from(y + 1);
                if target_pos.y >= ground_y
                    && target_pos.y - ground_y <= f64::from(AIRBORNE_TARGET_GROUND_SNAP_DISTANCE)
                {
                    return Vector3::new(target_pos.x, ground_y, target_pos.z);
                }
            }
        }

        target_pos
    }
}

impl Goal for MeleeAttackGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async {
            let time = {
                let world = mob.get_entity().world.load();
                let level_time = world.level_time.lock().await;
                level_time.world_age
            };

            if time - self.last_update_time < MAX_ATTACK_TIME {
                return false;
            }
            self.last_update_time = time;

            let target = mob.get_mob_entity().target.lock().await;

            let Some(target) = target.as_ref() else {
                return false;
            };
            if !target.get_entity().is_alive() {
                return false;
            }
            // TODO: add path when is implemented Navigation
            true //TODO: modify that because if a path to the target not exists then call mob.is_in_attack_range(target)
        })
    }

    fn should_continue<'a>(&'a self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async {
            let target = mob.get_mob_entity().target.lock().await.clone();

            let Some(target) = target else {
                return false;
            };
            if !target.get_entity().is_alive() {
                return false;
            }

            if !self.pause_when_mob_idle {
                let is_idle = mob
                    .get_mob_entity()
                    .navigator
                    .try_lock()
                    .is_ok_and(|navigator| navigator.is_idle());
                return !is_idle;
            }

            let is_valid_target = !target
                .get_player()
                .is_some_and(|p| p.is_spectator() || p.is_creative());

            let in_range = mob
                .get_mob_entity()
                .is_in_position_target_range_pos(&target.get_entity().block_pos.load());

            in_range && is_valid_target
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            // TODO: add missing fields like mob attacking to true and correct Navigation methods

            let target = mob.get_mob_entity().target.lock().await.clone();
            if let Some(target) = target {
                let mut navigator = mob.get_mob_entity().navigator.lock().unwrap();
                let target_pos = Self::navigation_target_position(target.as_ref());
                navigator.set_progress(NavigatorGoal {
                    current_progress: mob.get_entity().pos.load(),
                    destination: target_pos,
                    speed: self.speed,
                });
                self.last_target_position = Some(target_pos);
            }
            self.update_countdown_ticks = 0;
            self.cooldown = 0;
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            // Only clear target if they switched to creative/spectator
            let should_clear = {
                let target = mob.get_mob_entity().target.lock().await;
                if let Some(entity) = target.as_deref() {
                    !EntityPredicate::ExceptCreativeOrSpectator
                        .test(entity.get_entity())
                        .await
                } else {
                    false
                }
            };
            if should_clear {
                mob.set_mob_target(None).await;
            }

            // Vanilla: this.mob.getNavigation().stop()
            mob.get_mob_entity().navigator.lock().unwrap().stop();
            self.last_target_position = None;
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            let target = mob.get_mob_entity().target.lock().await.clone();
            let Some(target) = target else {
                return;
            };

            mob.get_mob_entity()
                .look_control
                .lock()
                .unwrap()
                .look_at_entity_with_range(&target, 30.0, 30.0);

            self.update_countdown_ticks = (self.update_countdown_ticks - 1).max(0);

            let current_target_pos = Self::navigation_target_position(target.as_ref());
            let in_attack_range = mob
                .get_mob_entity()
                .is_in_attack_range(target.as_ref())
                .await;

            if in_attack_range {
                // Keep the route alive but hold position. Calling `stop`
                // makes the non-idle melee goal terminate, allowing wander
                // to take over until target acquisition starts it again.
                mob.get_mob_entity().navigator.lock().unwrap().pause();
            } else {
                let navigator_is_idle = {
                    let mut navigator = mob.get_mob_entity().navigator.lock().unwrap();
                    navigator.resume();
                    navigator.is_idle()
                };
                let should_update_nav = navigator_is_idle
                    || (self.update_countdown_ticks <= 0
                        && (self.last_target_position.is_none_or(|last_pos| {
                            horizontal_distance_squared(current_target_pos, last_pos) >= 1.0
                        }) || mob.get_random().random_range(0..20) == 0));

                if should_update_nav {
                    let mob_pos = mob.get_entity().pos.load();
                    let dist_sq = mob_pos.squared_distance_to_vec(&current_target_pos);
                    let mut navigator = mob.get_mob_entity().navigator.lock().unwrap();
                    navigator.set_progress(NavigatorGoal {
                        current_progress: mob_pos,
                        destination: current_target_pos,
                        speed: self.speed,
                    });
                    self.last_target_position = Some(current_target_pos);
                    self.update_countdown_ticks = 4 + mob.get_random().random_range(0..7);
                    if dist_sq > 1024.0 {
                        self.update_countdown_ticks += 10;
                    } else if dist_sq > 256.0 {
                        self.update_countdown_ticks += 5;
                    }
                }
            }

            self.cooldown = (self.cooldown - 1).max(0);

            // TODO: Add visibility check (canSee) - requires world raycast
            if self.cooldown <= 0 && in_attack_range {
                self.cooldown = self.get_max_cooldown();
                mob.get_mob_entity().living_entity.swing_hand().await;
                mob.get_mob_entity().try_attack(mob, target.as_ref()).await;
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}
