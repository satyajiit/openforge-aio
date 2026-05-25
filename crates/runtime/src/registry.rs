//! The global registry: every game and feature compiled into the shipped
//! binary becomes a `&'static` entry here at link time.

use std::collections::HashMap;

use once_cell::sync::Lazy;
use tracing::{error, info, warn};

use crate::feature::{DeclarativeFeature, Feature};
use crate::game::Game;

pub struct GameEntry {
    pub game: &'static dyn Game,
}

pub struct CustomFeatureEntry {
    pub game_id: &'static str,
    pub feature: &'static dyn Feature,
}

inventory::collect!(GameEntry);
inventory::collect!(CustomFeatureEntry);

pub struct Registry {
    games: HashMap<&'static str, &'static dyn Game>,
    sorted_game_ids: Vec<&'static str>,
    features: HashMap<&'static str, Vec<&'static dyn Feature>>,
}

impl Registry {
    fn collect_internal() -> Self {
        let mut games: HashMap<&'static str, &'static dyn Game> = HashMap::new();
        for entry in inventory::iter::<GameEntry>() {
            let id = entry.game.id();
            if games.insert(id, entry.game).is_some() {
                error!(game_id = %id, "duplicate game registration; keeping the last one");
            }
        }

        let mut features: HashMap<&'static str, Vec<&'static dyn Feature>> = HashMap::new();
        for (&id, game) in &games {
            let bucket = features.entry(id).or_default();

            for src in game.declarative_features() {
                match DeclarativeFeature::from_toml(src.toml) {
                    Ok(df) => {
                        let leaked: &'static DeclarativeFeature = Box::leak(Box::new(df));
                        bucket.push(leaked as &'static dyn Feature);
                    }
                    Err(e) => {
                        error!(
                            game_id = %id,
                            sig = %src.name,
                            err = %e,
                            "failed to parse declarative feature; skipping"
                        );
                    }
                }
            }

            for &cf in game.custom_features() {
                bucket.push(cf);
            }
        }

        for entry in inventory::iter::<CustomFeatureEntry>() {
            features
                .entry(entry.game_id)
                .or_default()
                .push(entry.feature);
        }

        // Deduplicate feature ids per game; warn on collisions, keep the first.
        for (game_id, list) in features.iter_mut() {
            let mut seen: HashMap<String, usize> = HashMap::new();
            let mut keep_idx = Vec::new();
            for (i, f) in list.iter().enumerate() {
                let id = f.id().to_string();
                if let Some(&prev) = seen.get(&id) {
                    warn!(
                        game = %game_id,
                        feature = %id,
                        first_idx = prev,
                        dup_idx = i,
                        "duplicate feature id; keeping the first"
                    );
                } else {
                    seen.insert(id, i);
                    keep_idx.push(i);
                }
            }
            let kept: Vec<&'static dyn Feature> = keep_idx.into_iter().map(|i| list[i]).collect();
            *list = kept;
        }

        let mut sorted_game_ids: Vec<&'static str> = games.keys().copied().collect();
        sorted_game_ids.sort_by_key(|id| {
            let g = games[id];
            (g.sort_order(), g.display_name())
        });

        info!(games = games.len(), "registry collected");

        Self {
            games,
            sorted_game_ids,
            features,
        }
    }

    pub fn games(&self) -> impl Iterator<Item = &'static dyn Game> + '_ {
        self.sorted_game_ids
            .iter()
            .map(move |id| *self.games.get(id).unwrap())
    }

    pub fn game(&self, id: &str) -> Option<&'static dyn Game> {
        self.games.get(id).copied()
    }

    pub fn features_for(&self, game_id: &str) -> &[&'static dyn Feature] {
        self.features.get(game_id).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn feature(&self, game_id: &str, feature_id: &str) -> Option<&'static dyn Feature> {
        self.features_for(game_id)
            .iter()
            .find(|f| f.id() == feature_id)
            .copied()
    }
}

pub static REGISTRY: Lazy<Registry> = Lazy::new(Registry::collect_internal);

#[macro_export]
macro_rules! register_game {
    ($t:ident) => {
        $crate::inventory::submit! {
            $crate::GameEntry {
                game: {
                    static G: $t = $t;
                    &G
                }
            }
        }
    };
}

#[macro_export]
macro_rules! register_feature {
    ($game_id:expr, $t:ident) => {
        $crate::inventory::submit! {
            $crate::CustomFeatureEntry {
                game_id: $game_id,
                feature: {
                    static F: $t = $t;
                    &F
                }
            }
        }
    };
}
