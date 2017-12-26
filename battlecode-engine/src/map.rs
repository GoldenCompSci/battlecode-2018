//! The starting properties of the game world.

use std::f32;
use failure::Error;
use fnv::FnvHashMap;
use rand::{SeedableRng, StdRng};
use rand::distributions::IndependentSample;
use rand::distributions::range::Range;

use constants::*;
use error::GameError;
use location::*;
use unit::Unit;

/// The map defining the starting state for an entire game.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GameMap {
    /// Seed for random number generation.
    pub seed: u32,
    /// Earth map.
    pub earth_map: PlanetMap,
    /// Mars map.
    pub mars_map: PlanetMap,
    /// Weather pattern.
    pub weather: WeatherPattern,
}

impl GameMap {
    /// Validate the game map.
    pub fn validate(&self) -> Result<(), Error> {
        self.earth_map.validate()?;
        self.mars_map.validate()?;
        self.weather.validate()?;
        Ok(())
    }
}

/// The map for one of the planets in the Battlecode world. This information
/// defines the terrain, dimensions, and initial units of the planet.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanetMap {
    /// The planet of the map.
    pub planet: Planet,

    /// The height of this map, in squares. Must be in the range
    /// [constants::MAP_HEIGHT_MIN, constants::MAP_HEIGHT_MAX], inclusive.
    pub height: usize,

    /// The width of this map, in squares. Must be in the range
    /// [constants::MAP_WIDTH_MIN, constants::MAP_WIDTH_MAX], inclusive.
    pub width: usize,

    /// The coordinates of the bottom-left corner. Essentially, the
    /// minimum x and y coordinates for this map. Each lies within
    /// [constants::MAP_COORDINATE_MIN, constants::MAP_COORDINATE_MAX],
    /// inclusive.
    pub origin: MapLocation,

    /// Whether the specified square contains passable terrain. Is only
    /// false when the square contains impassable terrain (distinct from
    /// containing a building, for instance).
    ///
    /// Stored as a two-dimensional array, where the first index 
    /// represents a square's y-coordinate, and the second index its 
    /// x-coordinate. These coordinates are *relative to the origin*.
    ///
    /// Earth is always symmetric by either a rotation or a reflection.
    pub is_passable_terrain: Vec<Vec<bool>>,

    /// The amount of Karbonite deposited on the specified square.
    ///
    /// Stored as a two-dimensional array, where the first index 
    /// represents a square's y-coordinate, and the second index its 
    /// x-coordinate. These coordinates are *relative to the origin*.
    pub initial_karbonite: Vec<Vec<u32>>,

    /// The initial units on the map. Each team starts with 1 to 3 Workers
    /// on Earth. The coordinates of the units are absolute (NOT relative to
    /// the origin).
    pub initial_units: Vec<Unit>,
}

impl PlanetMap {
    /// Validates the map and checks some invariants are followed.
    pub fn validate(&self) -> Result<(), Error> {
        // The width and height are of valid dimensions.
        if !(self.height >= MAP_HEIGHT_MIN && self.height <= MAP_HEIGHT_MAX &&
             self.width >= MAP_WIDTH_MIN && self.width <= MAP_WIDTH_MAX) {
            Err(GameError::InvalidMapObject)?
        }

        // The origin is valid.
        if !(self.origin.x >= MAP_COORDINATE_MIN &&
             self.origin.x <= MAP_COORDINATE_MAX &&
             self.origin.y >= MAP_COORDINATE_MIN &&
             self.origin.y <= MAP_COORDINATE_MAX &&
             self.origin.planet == self.planet) {
            Err(GameError::InvalidMapObject)?
        }

        // The terrain definition is valid.
        if self.is_passable_terrain.len() != self.height ||
           self.is_passable_terrain[0].len() != self.width {
            Err(GameError::InvalidMapObject)?
        }

        // The initial karbonite deposits are valid.
        if self.initial_karbonite.len() != self.height ||
           self.initial_karbonite[0].len() != self.width {
            Err(GameError::InvalidMapObject)?
        }
        for y in 0..self.height {
            for x in 0..self.width {
                match self.planet {
                    Planet::Mars => {
                        if self.initial_karbonite[y][x] != 0 {
                            Err(GameError::InvalidMapObject)?
                        }
                    }
                    Planet::Earth => {
                        if self.initial_karbonite[y][x] < MAP_KARBONITE_MIN ||
                           self.initial_karbonite[y][x] > MAP_KARBONITE_MAX {
                            Err(GameError::InvalidMapObject)?
                        }
                    }
                }
            }
        }

        // The initial units are valid.
        let num_units = self.initial_units.len();
        match self.planet {
            Planet::Mars => {
                if num_units != 0 {
                    Err(GameError::InvalidMapObject)?
                }
            }
            Planet::Earth => {
                if !(num_units > 0 && num_units % 2 == 0 && num_units <= 6) {
                    Err(GameError::InvalidMapObject)?
                }
            }
        }
        for ref unit in &self.initial_units {
            let x = (unit.location.x - self.origin.x) as usize;
            let y = (unit.location.y - self.origin.y) as usize;
            if unit.location.planet != self.planet {
                Err(GameError::InvalidMapObject)?
            }
            if !self.is_passable_terrain[y][x] {
                Err(GameError::InvalidMapObject)?
            }
        }

        // The map is symmetric on Earth.
        if self.planet == Planet::Earth {
            // TODO
        }
        Ok(())
    }

    /// Whether a location is on the map.
    pub fn on_map(&self, location: &MapLocation) -> bool {
        self.planet == location.planet
            && location.x >= self.origin.x
            && location.y >= self.origin.y
            && location.x < self.origin.x + self.width as i32
            && location.y < self.origin.y + self.height as i32
    }

    pub fn test_map(planet: Planet) -> PlanetMap {
        PlanetMap {
            planet: planet,
            height: MAP_HEIGHT_MIN,
            width: MAP_WIDTH_MIN,
            origin: MapLocation::new(planet, 0, 0),
            is_passable_terrain: vec![vec![true; MAP_WIDTH_MIN]; MAP_HEIGHT_MIN],
            initial_karbonite: vec![vec![0; MAP_WIDTH_MIN]; MAP_HEIGHT_MIN],
            initial_units: vec![],
        }
    }
}

/// A single asteroid strike on Mars.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct AsteroidStrike {
    /// The karbonite on the asteroid.
    pub karbonite: u32,
    /// The location of the strike.
    pub location: MapLocation,
}

/// The round number to an asteroid strike.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AsteroidPattern {
    pattern: FnvHashMap<u32, AsteroidStrike>,
}

/// The orbit pattern that determines a rocket's flight duration. This pattern
/// is a sinusoidal function y=a*sin(bx)+c.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrbitPattern {
    /// Amplitude of the orbit, in rounds.
    pub amplitude: i32,
    /// The period of the orbit, in rounds.
    pub period: i32,
    /// The center of the orbit, in rounds.
    pub center: i32,
}

impl AsteroidStrike {
    /// Constructs a new asteroid strike.
    pub fn new(karbonite: u32, location: MapLocation) -> AsteroidStrike {
        AsteroidStrike {
            karbonite: karbonite,
            location: location,
        }
    }
}

impl AsteroidPattern {
    /// Constructs a new asteroid pattern from a map of round number to strike.
    pub fn new(pattern: &FnvHashMap<u32, AsteroidStrike>) -> AsteroidPattern {
        AsteroidPattern {
            pattern: pattern.clone(),
        }
    }

    /// Constructs a pseudorandom asteroid pattern given a map of Mars.
    pub fn random(seed: u32, mars_map: &PlanetMap) -> AsteroidPattern {
        let mut pattern: FnvHashMap<u32, AsteroidStrike> = FnvHashMap::default();

        let karbonite_gen = Range::new(ASTEROID_KARB_MIN, ASTEROID_KARB_MAX);
        let round_gen = Range::new(ASTEROID_ROUND_MIN, ASTEROID_ROUND_MAX);
        let x_gen = Range::new(mars_map.origin.x, mars_map.origin.x + mars_map.width as i32);
        let y_gen = Range::new(mars_map.origin.y, mars_map.origin.y + mars_map.height as i32);

        let seed: &[_] = &[seed as usize];
        let mut rng: StdRng = SeedableRng::from_seed(seed);
        let mut round = 0;
        loop {
            round += round_gen.ind_sample(&mut rng);
            if round >= ROUND_LIMIT {
                break;
            }
            pattern.insert(round, AsteroidStrike {
                karbonite: karbonite_gen.ind_sample(&mut rng),
                location: MapLocation {
                    planet: Planet::Mars,
                    x: x_gen.ind_sample(&mut rng),
                    y: y_gen.ind_sample(&mut rng),
                },
            });
        }

        AsteroidPattern {
            pattern: pattern,
        }
    }

    /// Validates the asteroid pattern.
    pub fn validate(&self) -> Result<(), Error> {
        // The Karbonite on each asteroid is in the range
        // [ASTEROID_KARB_MIN, ASTEROID_KARB_MAX], inclusive.
        for (round, asteroid) in self.pattern.clone() {
            if round < 1 || round > ROUND_LIMIT {
                Err(GameError::InvalidMapObject)?
            }
            if asteroid.karbonite < ASTEROID_KARB_MIN ||
               asteroid.karbonite > ASTEROID_KARB_MAX {
                Err(GameError::InvalidMapObject)?
            }
            if asteroid.location.planet != Planet::Mars {
                Err(GameError::InvalidMapObject)?
            }
        }

        // An asteroid strikes every [ASTEROID_ROUND_MIN,
        // ASTEROID_ROUND_MAX] rounds, inclusive.
        let mut rounds: Vec<&u32> = self.pattern.keys().collect();
        rounds.sort();
        if rounds[0] - 1 > ASTEROID_ROUND_MAX {
            Err(GameError::InvalidMapObject)?
        }
        if ROUND_LIMIT - rounds[rounds.len() - 1] > ASTEROID_ROUND_MAX {
            Err(GameError::InvalidMapObject)?
        }
        for i in 0..rounds.len() - 1 {
            let diff = rounds[i + 1] - rounds[i];
            if diff < ASTEROID_ROUND_MIN || diff > ASTEROID_ROUND_MAX {
                Err(GameError::InvalidMapObject)?
            }
        }
        Ok(())
    }

    /// Get the asteroid strike at the given round.
    pub fn get_asteroid(&self, round: u32) -> Option<&AsteroidStrike> {
        self.pattern.get(&round)
    }

    /// Get a map of round numbers to asteroid strikes.
    pub fn get_asteroid_map(&self) -> FnvHashMap<u32, AsteroidStrike> {
        self.pattern.clone()
    }
}

impl OrbitPattern {
    /// Construct a new orbit pattern. This pattern is a sinusoidal function
    /// y=a*sin(bx)+c, where the x-axis is the round number of takeoff and the
    /// the y-axis is the flight of duration to the nearest integer.
    ///
    /// The amplitude, period, and center are measured in rounds.
    pub fn new(amplitude: i32, period: i32, center: i32) -> OrbitPattern {
        OrbitPattern {
            amplitude: amplitude,
            period: period,
            center: center,
        }
    }

    /// Validates the orbit pattern.
    pub fn validate(&self) -> Result<(), Error> {
        // The flight times are within [ORIBIT_FLIGHT_MIN, ORBIT_FLIGHT_MAX].
        if self.center - self.amplitude < ORBIT_FLIGHT_MIN {
            Err(GameError::InvalidMapObject)?
        }
        if self.center + self.amplitude > ORBIT_FLIGHT_MAX {
            Err(GameError::InvalidMapObject)?
        }
        Ok(())
    }

    /// Get the duration of flight if the rocket were to take off from either
    /// planet on the given round.
    pub fn get_duration(&self, round: i32) -> i32 {
        let arg = 2. * f32::consts::PI / self.period as f32 * round as f32;
        ((self.amplitude as f32) * f32::sin(arg)) as i32 + self.center
    }
}

/// The weather patterns defined in the game world.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WeatherPattern {
    /// The asteroid strike pattern on Mars.
    pub asteroids: AsteroidPattern,
    /// The orbit pattern that determines a rocket's flight duration.
    pub orbit: OrbitPattern,
}

impl WeatherPattern {
    /// Construct a new weather pattern.
    pub fn new(asteroids: AsteroidPattern, orbit: OrbitPattern) -> WeatherPattern {
        WeatherPattern {
            asteroids: asteroids,
            orbit: orbit,
        }
    }

    /// Validate the weather pattern.
    pub fn validate(&self) -> Result<(), Error> {
        self.asteroids.validate()?;
        self.orbit.validate()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use fnv::FnvHashMap;

    use super::AsteroidPattern;
    use super::AsteroidStrike;
    use super::OrbitPattern;
    use super::super::constants::*;
    use super::super::location::*;

    fn insert_and_err(pattern: &FnvHashMap<u32, AsteroidStrike>,
                      round: u32, karbonite: u32, location: MapLocation) {
        let mut invalid = pattern.clone();
        invalid.insert(round, AsteroidStrike::new(karbonite, location));
        assert!(AsteroidPattern::new(&invalid).validate().is_err());
    }

    fn gen_asteroid_map(start_round: u32, skip_round: u32) -> FnvHashMap<u32, AsteroidStrike> {
        let mut asteroid_map: FnvHashMap<u32, AsteroidStrike> = FnvHashMap::default();
        let mut round = start_round;
        let default_loc = MapLocation::new(Planet::Mars, 0, 0);
        let default_strike = AsteroidStrike::new(ASTEROID_KARB_MIN, default_loc);
        while round <= ROUND_LIMIT {
            asteroid_map.insert(round, default_strike.clone());
            round += skip_round;
        }
        asteroid_map
    }

    #[test]
    fn validate_asteroid() {
        // Valid randomly-generated asteroid patterns.
        let ref mars_map = super::PlanetMap::test_map(Planet::Mars);
        for seed in 0..5 {
            AsteroidPattern::random(seed, mars_map).validate().is_ok();
        }

        // Generate an asteroid pattern from a map.
        let asteroid_map = AsteroidPattern::random(0, mars_map).get_asteroid_map();
        let asteroids = AsteroidPattern::new(&asteroid_map);
        asteroids.validate().is_ok();

        let mut asteroid_map = gen_asteroid_map(1, ASTEROID_ROUND_MAX);
        AsteroidPattern::new(&asteroid_map).validate().is_ok();

        // Invalid asteroid strikes.
        let loc = MapLocation::new(Planet::Mars, 0, 0);
        insert_and_err(&asteroid_map, 0, ASTEROID_KARB_MIN, loc);
        insert_and_err(&asteroid_map, ROUND_LIMIT + 1, ASTEROID_KARB_MIN, loc);
        insert_and_err(&asteroid_map, 1, ASTEROID_KARB_MIN - 1, loc);
        insert_and_err(&asteroid_map, 1, ASTEROID_KARB_MAX + 1, loc);
        insert_and_err(&asteroid_map, 1, ASTEROID_KARB_MAX, MapLocation::new(Planet::Earth, 0, 0));

        // Invalid strike pattern.
        insert_and_err(&asteroid_map, 2, ASTEROID_KARB_MIN, loc);
        asteroid_map.remove(&ASTEROID_ROUND_MIN);
        AsteroidPattern::new(&asteroid_map).validate().is_err();
    }

    #[test]
    fn validate_orbit() {
        assert!(OrbitPattern::new(150, 200, 200).validate().is_err());
        assert!(OrbitPattern::new(150, 200, 300).validate().is_err());
        assert!(OrbitPattern::new(150, 200, 250).validate().is_ok());
    }

    #[test]
    fn get_asteroid() {
        let asteroid_map = gen_asteroid_map(ASTEROID_ROUND_MAX, ASTEROID_ROUND_MAX);
        let asteroids = AsteroidPattern::new(&asteroid_map);
        for round in 1..ROUND_LIMIT {
            if round % ASTEROID_ROUND_MAX == 0 {
                assert!(asteroids.get_asteroid(round).is_some());
            } else {
                assert!(asteroids.get_asteroid(round).is_none());
            }
        }
    }

    #[test]
    fn get_duration() {
        let period = 200;
        let orbit = OrbitPattern::new(150, period, 250);
        for i in 0..5 {
            let base = period * i;
            assert_eq!(250, orbit.get_duration(base));
            assert_eq!(400, orbit.get_duration(base + period / 4));
            assert_eq!(250, orbit.get_duration(base + period / 2));
            assert_eq!(100, orbit.get_duration(base + period * 3 / 4));
            assert_eq!(250, orbit.get_duration(base + period));

            let duration = orbit.get_duration(base + period / 8);
            assert!(duration > 250 && duration < 400);
        }
    }
}
