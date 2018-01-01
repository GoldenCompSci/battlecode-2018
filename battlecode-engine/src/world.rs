//! The core battlecode engine.

use std::collections::HashSet;
use fnv::FnvHashMap;

use super::constants::*;
use super::schema::Delta;
use super::id_generator::IDGenerator;
use super::location::*;
use super::location::Location::*;
use super::map::*;
use super::unit::*;
use super::unit::UnitType as Branch;
use super::research::*;
use super::error::GameError;
use failure::Error;

/// A round consists of a turn from each player.
pub type Rounds = u32;

/// A rocket landing.
type RocketLanding = (UnitID, MapLocation);

/// All rocket landings.
type RocketLandingInfo = FnvHashMap<Rounds, Vec<RocketLanding>>;

/// There are two teams in Battlecode: Red and Blue.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub enum Team {
    Red,
    Blue,
}

impl Team {
    /// The other team.
    pub fn other(&self) -> Team {
        match *self {
            Team::Red => Team::Blue,
            Team::Blue => Team::Red,
        }
    }
}

/// The state for one of the planets in a game. Stores neutral info like the
/// planet's original map and the current karbonite deposits.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct PlanetInfo {
    /// The map of the planet.
    map: PlanetMap,

    /// The amount of Karbonite deposited on the specified square.
    ///
    /// Stored as a two-dimensional array, where the first index 
    /// represents a square's y-coordinate, and the second index its 
    /// x-coordinate.
    karbonite: Vec<Vec<u32>>,
}

impl PlanetInfo {
    /// Construct a planet with the given map, where the current karbonite
    /// deposits are initialized with the map's initial deposits.
    pub fn new(map: PlanetMap) -> PlanetInfo {
        let karbonite = map.initial_karbonite.clone();
        PlanetInfo {
            map: map,
            karbonite: karbonite,
        }
    }
}

/// A team-shared communication array.
pub type TeamArray = Vec<u8>;

/// A history of communication arrays. Read from the back of the queue on the
/// current planet, and the front of the queue on the other planet.
type TeamArrayHistory = Vec<TeamArray>;

/// Persistent info specific to a single team. Teams are only able to access
/// the team info of their own team.
#[derive(Debug, Serialize, Deserialize, Clone)]
struct TeamInfo {
    /// Team identification.
    team: Team,

    /// Unit ID generator.
    id_generator: IDGenerator,

    /// Communication array histories for each planet.
    team_arrays: FnvHashMap<Planet, TeamArrayHistory>,

    /// The current state of research.
    research: ResearchInfo,

    /// The karbonite in the team's resource pool.
    karbonite: u32,
}

impl TeamInfo {
    /// Construct a team with the default properties.
    fn new(team: Team, seed: u32) -> TeamInfo {
        TeamInfo {
            team: team,
            id_generator: IDGenerator::new(team, seed),
            team_arrays: FnvHashMap::default(),
            research: ResearchInfo::new(),
            karbonite: KARBONITE_STARTING,
        }
    }
}

/// A player represents a program controlling some group of units.
#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub struct Player {
    /// The team of this player.
    pub team: Team,

    /// The planet for this player. Each team disjointly controls the robots on each planet.
    pub planet: Planet,
}

impl Player {
    /// Constructs a new player.
    pub fn new(team: Team, planet: Planet) -> Player {
        Player { team: team, planet: planet }
    }
}

/// The full world of the Battlecode game.
///
/// The contents of the game world differ depending on whether it exists in the
/// Teh Devs engine or the Player engine. Do not be concerned - this ensures
/// that your player the visibility it's supposed to have!
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GameWorld {
    /// The current round, starting at 1.
    round: Rounds,

    /// The player whose turn it is.
    player_to_move: Player,

    /// Locations visible to the team.
    visible_locs: HashSet<MapLocation>,

    /// The unit controllers in the vision range. Every unit has a unit info.
    units: FnvHashMap<UnitID, Unit>,

    /// The units in the vision range. Not every unit info may have a unit.
    unit_infos: FnvHashMap<UnitID, UnitInfo>,

    /// All the units on the map, by map location. Cached for performance.
    units_by_loc: FnvHashMap<MapLocation, UnitID>,

    /// Rocket landings. Maps round numbers to a list of rockets
    /// landing on the given round.
    rocket_landings: RocketLandingInfo,

    /// The asteroid strike pattern on Mars.
    asteroids: AsteroidPattern,

    /// The orbit pattern that determines a rocket's flight duration.
    orbit: OrbitPattern,

    /// The state of each planet.
    planet_states: FnvHashMap<Planet, PlanetInfo>,

    /// The state of each team.
    team_states: FnvHashMap<Team, TeamInfo>,
}

impl GameWorld {
    /// Initialize a new game world with maps from both planets.
    ///
    /// * GameError::InvalidMapObject - the map is invalid, check the specs.
    pub fn new(map: GameMap) -> Result<GameWorld, Error> {
        map.validate()?;

        let mut planet_states = FnvHashMap::default();
        planet_states.insert(Planet::Earth, PlanetInfo::new(map.earth_map));
        planet_states.insert(Planet::Mars, PlanetInfo::new(map.mars_map));

        let mut team_states = FnvHashMap::default();
        team_states.insert(Team::Red, TeamInfo::new(Team::Red, map.seed));
        team_states.insert(Team::Blue, TeamInfo::new(Team::Blue, map.seed));

        Ok(GameWorld {
            round: 1,
            player_to_move: Player { team: Team::Red, planet: Planet::Earth },
            visible_locs: HashSet::default(),
            units: FnvHashMap::default(),
            unit_infos: FnvHashMap::default(),
            units_by_loc: FnvHashMap::default(),
            rocket_landings: FnvHashMap::default(),
            asteroids: map.asteroids,
            orbit: map.orbit,
            planet_states: planet_states,
            team_states: team_states,
        })
    }

    /// Filters the game world from the perspective of the current player. All
    /// units are within the player's vision range. Information on the opposing
    /// player's units are all stored in `UnitInfo` structs, not `Unit`. Private
    /// player information like communication arrays and rockets in space should
    /// only be stored for the current player.
    ///
    /// As an invariant, the game world filtered once should be the same as the
    /// game world filtered multiple times.
    pub fn filter(&self) -> Result<GameWorld, Error> {
        let team = self.team();
        let planet = self.planet();
        let map = self.starting_map(planet);

        // Filter the unit controllers, excluding the units in rockets.
        let mut units = self.units.values().clone().into_iter()
           .filter(|unit| unit.team() == team
            && (unit.location().on_planet(planet) || unit.location() == InSpace))
           .collect::<Vec<&Unit>>();

        // Calculate the visible locations on this team that are on the map.
        let mut visible_locs: HashSet<MapLocation> = HashSet::default();
        for ref unit in units.clone() {
            if unit.location() == InSpace {
                continue;
            }

            for loc in unit.location().map_location().expect("unit is not on the map")
                           .all_locations_within(unit.vision_range())
                           .expect("vision range is too large")
                           .into_iter()
                           .filter(|loc| map.on_map(*loc)) {
                visible_locs.insert(loc);
            }
        }

        // Filter the unit infos.
        let unit_infos = self.unit_infos.clone().into_iter()
            .filter(|&(_, unit)| unit.location.on_map() && visible_locs.contains(
                &unit.location.map_location().expect("unit must be on map"))
            )
            .collect::<FnvHashMap<UnitID, UnitInfo>>();

        // Add the units in rockets to the list of unit controllers.
        let units_in_rockets = units.clone().into_iter()
            .filter(|unit| unit.unit_type() == UnitType::Rocket)
            .map(|rocket| rocket.garrisoned_units()
                .expect("unit must be a rocket").into_iter()
                .map(|id| self.get_unit(id).expect("unit does not exist"))
                .collect::<Vec<&Unit>>()
            );
        for rocket_units in units_in_rockets {
            units.extend(rocket_units);
        }

        // Create units by location.
        let units_by_loc = units.clone().into_iter()
            .filter(|unit| unit.location().on_map())
            .map(|unit| (unit.location().map_location().expect("unit must be on the map"), unit.id()))
            .collect::<FnvHashMap<MapLocation, UnitID>>();

        // Turn the units back into a hash map.
        let units = units.into_iter()
            .map(|unit| (unit.id(), unit.clone()))
            .collect::<FnvHashMap<UnitID, Unit>>();

        // Filter the rocket landings.
        let mut rocket_landings: RocketLandingInfo = FnvHashMap::default();
        for (round, landings) in self.rocket_landings.clone().into_iter() {
            let my_landings = landings.into_iter()
                .filter(|&(id, _)| self.get_unit(id).expect("unit must exist").team() == team)
                .collect::<Vec<RocketLanding>>();
            if my_landings.len() > 0 {
                rocket_landings.insert(round, my_landings);
            }
        }

        // Filter the team states.
        let mut team_states: FnvHashMap<Team, TeamInfo> = FnvHashMap::default();
        team_states.insert(team, self.get_team_info(team).clone());

        Ok(GameWorld {
            round: self.round,
            player_to_move: self.player_to_move,
            visible_locs: visible_locs,
            units: units,
            unit_infos: unit_infos,
            units_by_loc: units_by_loc,
            rocket_landings: rocket_landings,
            asteroids: self.asteroids.clone(),
            orbit: self.orbit.clone(),
            planet_states: self.planet_states.clone(),
            team_states: team_states,
        })
    }

    // ************************************************************************
    // ************************** GENERAL METHODS *****************************
    // ************************************************************************

    /// The current round, starting at round 1 and up to `ROUND_LIMIT` rounds.
    /// A round consists of a turn from each team on each planet.
    pub fn round(&self) -> Rounds {
        self.round
    }

    /// The current planet.
    pub fn planet(&self) -> Planet {
        self.player_to_move.planet
    }

    /// The team whose turn it is.
    pub fn team(&self) -> Team {
        self.player_to_move.team
    }

    /// The rockets in space that belong to the current team, including
    /// their landing rounds and locations, by landing round.
    pub fn rockets_in_space(&self) -> FnvHashMap<Rounds, Vec<Unit>> {
        unimplemented!();
    }

    /// The starting map of the given planet. Includes the map's planet,
    /// dimensions, impassable terrain, and initial units and karbonite.
    pub fn starting_map(&self, planet: Planet) -> &PlanetMap {
        &self.get_planet_info(planet).map
    }

    /// The karbonite in the team's resource pool.
    pub fn karbonite(&self) -> u32 {
        let team = self.team();
        self.get_team_info(team).karbonite
    }

    // ************************************************************************
    // ************************** SENSING METHODS *****************************
    // ************************************************************************

    /// The unit controller for the unit of this ID. Use this method to get
    /// detailed statistics on a unit in your team: heat, cooldowns, and
    /// properties of special abilities like units garrisoned in a rocket.
    ///
    /// Note that mutating this object does NOT have any effect on the actual
    /// game. You MUST call the mutators in world!!
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    pub fn unit(&self, _id: UnitID) -> Result<&Unit, Error> {
        unimplemented!();
    }

    /// All the units within the vision range.
    pub fn units(&self) -> Vec<UnitInfo> {
        unimplemented!();
    }

    /// All the units within the vision range, by ID.
    pub fn units_by_id(&self) -> FnvHashMap<UnitID, UnitInfo> {
        unimplemented!();
    }

    /// All the units within the vision range, by location.
    pub fn units_by_loc(&self) -> FnvHashMap<MapLocation, UnitID> {
        unimplemented!();
    }

    /// The karbonite at the given location.
    ///
    /// * GameError::InvalidLocation - the location is outside the vision range.
    pub fn karbonite_at(&self, _location: MapLocation) -> Result<u32, Error> {
        unimplemented!();
    }

    /// Whether the location is within the vision range.
    pub fn can_sense_location(&self, _location: MapLocation) -> bool {
        unimplemented!();
    }

    /// Whether there is a unit with this ID within the vision range.
    pub fn can_sense_unit(&self, _id: UnitID) -> bool {
        unimplemented!();
    }

    /// Sense units near the location within the given radius, inclusive, in
    /// distance squared. The units are within the vision range.
    pub fn sense_nearby_units(&self, _location: MapLocation, _radius: u32)
                              -> Vec<UnitInfo> {
        unimplemented!();
    }

    /// Sense units near the location within the given radius, inclusive, in
    /// distance squared. The units are within the vision range. Additionally
    /// filters the units by team.
    pub fn sense_nearby_units_by_team(&self, _location: MapLocation,
                                      _radius: u32, _team: Team) -> Vec<UnitInfo> {
        unimplemented!();
    }

    /// Sense units near the location within the given radius, inclusive, in
    /// distance squared. The units are within the vision range. Additionally
    /// filters the units by unit type.
    pub fn sense_nearby_units_by_type(&self, _location: MapLocation,
                                      _radius: u32, _type: UnitType) -> Vec<UnitInfo> {
        unimplemented!();
    }

    /// The unit at the location, if it exists.
    ///
    /// * GameError::InvalidLocation - the location is outside the vision range.
    pub fn sense_unit_at_location(&self, _location: MapLocation)
                                  -> Result<Option<UnitInfo>, Error> {
        unimplemented!();
    }

    // ************************************************************************
    // ************************** WEATHER METHODS *****************************
    // ************************************************************************

    /// The asteroid strike pattern on Mars.
    pub fn asteroid_pattern(&self) -> AsteroidPattern {
        self.asteroids.clone()
    }

    /// The orbit pattern that determines a rocket's flight duration.
    pub fn orbit_pattern(&self) -> OrbitPattern {
        self.orbit.clone()
    }

    /// The current duration of flight if a rocket were to be launched this
    /// round. Does not take into account any research done on rockets.
    pub fn current_duration_of_flight(&self) -> Rounds {
        self.orbit.duration(self.round)
    }

    fn process_asteroids(&mut self) {
        if self.asteroids.asteroid(self.round).is_some() {
            let (location, karbonite) = {
                let asteroid = self.asteroids.asteroid(self.round).unwrap();
                (asteroid.location, asteroid.karbonite)
            };
            let planet_info = self.get_planet_info_mut(location.planet);
            planet_info.karbonite[location.y as usize][location.x as usize] += karbonite;
        }
    }

    // ************************************************************************
    // *********************** COMMUNICATION METHODS **************************
    // ************************************************************************

    // ************************************************************************
    // ****************************** ACCESSORS *******************************
    // ************************************************************************

    fn get_planet_info(&self, planet: Planet) -> &PlanetInfo {
        if let Some(planet_info) = self.planet_states.get(&planet) {
            planet_info
        } else {
            unreachable!();
        }
    }
    
    fn get_planet_info_mut(&mut self, planet: Planet) -> &mut PlanetInfo {
        if let Some(planet_info) = self.planet_states.get_mut(&planet) {
            planet_info
        } else {
            unreachable!();
        }
    }

    fn get_team_info(&self, team: Team) -> &TeamInfo {
        if let Some(team_info) = self.team_states.get(&team) {
            team_info
        } else {
            unreachable!();
        }
    }

    fn get_team_info_mut(&mut self, team: Team) -> &mut TeamInfo {
        if let Some(team_info) = self.team_states.get_mut(&team) {
            team_info
        } else {
            unreachable!();
        }
    }

    fn get_unit(&self, id: UnitID) -> Result<&Unit, Error> {
        if let Some(unit) = self.units.get(&id) {
            Ok(unit)
        } else {
            Err(GameError::NoSuchUnit)?
        }
    }

    fn get_unit_mut(&mut self, id: UnitID) -> Result<&mut Unit, Error> {
        if let Some(unit) = self.units.get_mut(&id) {
            Ok(unit)
        } else {
            Err(GameError::NoSuchUnit)?
        }
    }

    // ************************************************************************
    // **************** UNIT CREATION / DESTRUCTION METHODS *******************
    // ************************************************************************

    /// Places a unit onto the map at the given location. Assumes the given square is occupiable.
    fn place_unit(&mut self, id: UnitID) {
        match self.get_unit(id)
                  .expect("Unit is not on a map and cannot be removed.")
                  .location() {
            OnMap(map_loc) => {
                self.units_by_loc.insert(map_loc, id);
            },
            _ => panic!("Unit is not on a map and cannot be placed."),
        }
    }

    fn remove_unit(&mut self, id: UnitID) {
        match self.get_unit(id)
                  .expect("Unit is not on a map and cannot be removed.")
                  .location() {
            OnMap(loc) => {
                self.units_by_loc.remove(&loc);
            },
            _ => panic!("Unit is not on a map and cannot be removed."),
        }
    }

    /// Creates and inserts a new unit into the game world, so that it can be
    /// referenced by ID.
    fn create_unit(&mut self, team: Team, location: MapLocation,
                       unit_type: UnitType) -> Result<UnitID, Error> {
        let id = self.get_team_info_mut(team).id_generator.next_id();
        let level = self.get_team_info(team).research.get_level(&unit_type);
        let unit = Unit::new(id, team, unit_type, level, location)?;

        self.units.insert(unit.id(), unit);
        self.place_unit(id);
        Ok(id)
    }

    /// Deletes a unit. Unit should be removed from map first.
    fn delete_unit(&mut self, id: UnitID) {
        self.units.remove(&id);
    }

    /// Destroys a unit.
    fn destroy_unit(&mut self, id: UnitID) -> Result<(), Error> {
        match self.get_unit(id)?.location() {
            OnMap(loc) => {
                self.units_by_loc.remove(&loc);
                self.get_unit_mut(id)?.destroy();
            },
            _ => {},
        }
        if self.get_unit(id)?.unit_type() == UnitType::Rocket {
            let units_to_destroy = self.get_unit_mut(id)?.garrisoned_units()?;
            for utd_id in units_to_destroy.iter() {
                self.destroy_unit(*utd_id)?;
            }
        }
        self.delete_unit(id);
        Ok(())
    }

    /// Disintegrates the unit and removes it from the map. If the unit is a
    /// factory or a rocket, also disintegrates any units garrisoned inside it.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    pub fn disintegrate_unit(&mut self, _id: UnitID) -> Result<(), Error> {
        unimplemented!();
    }

    // ************************************************************************
    // ************************* LOCATION METHODS *****************************
    // ************************************************************************

    /// Whether the location is clear for a unit to occupy, either by movement
    /// or by construction.
    ///
    /// * GameError::InvalidLocation - the location is outside the vision range.
    pub fn is_occupiable(&self, location: MapLocation) -> Result<bool, Error> {
        let planet_info = &self.get_planet_info(location.planet);
        Ok(planet_info.map.is_passable_terrain_at(location)? &&
            !self.units_by_loc.contains_key(&location))
    }

    /// Whether the robot can move in the given direction, without taking into
    /// account the unit's movement heat. Takes into account only the map
    /// terrain, positions of other robots, and the edge of the game map.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a robot.
    pub fn can_move(&self, robot_id: UnitID, direction: Direction) -> Result<bool, Error> {
        // TODO: should return false if off the map
        let unit = self.get_unit(robot_id)?;
        if let OnMap(location) = unit.location() {
            Ok(unit.is_move_ready()? && self.is_occupiable(location.add(direction))?)
        } else {
            Ok(false)
        }
    }

    /// Whether the robot is ready to move. Tests whether the robot's attack
    /// heat is sufficiently low.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a robot.
    pub fn is_move_ready(&self, _robot_id: UnitID) -> Result<bool, Error> {
        unimplemented!();
    }

    /// Moves the robot in the given direction.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a robot.
    /// * GameError::InvalidAction - the robot cannot move in that direction.
    pub fn move_robot(&mut self, robot_id: UnitID, direction: Direction) -> Result<(), Error> {
        if self.can_move(robot_id, direction)? {
            let dest = match self.get_unit(robot_id)?.location() {
                OnMap(loc) => loc.add(direction),
                _ => Err(GameError::InvalidAction)?,
            };
            self.remove_unit(robot_id);
            self.get_unit_mut(robot_id)?.move_to(dest)?;
            self.place_unit(robot_id);
            Ok(())
        } else {
            Err(GameError::InvalidAction)?
        }
    }

    // ************************************************************************
    // *************************** ATTACK METHODS *****************************
    // ************************************************************************

    /// Deals damage to any unit in the target square, potentially destroying it.
    fn damage_location(&mut self, location: MapLocation, damage: i32) -> Result<(), Error> {
        let id = if let Some(id) = self.units_by_loc.get(&location) {
            *id
        } else {
            return Ok(());
        };

        let should_destroy_unit = self.get_unit_mut(id)?.take_damage(damage);
        if should_destroy_unit {
            self.destroy_unit(id)?;
        }
        Ok(())
    }

    /// Whether the robot can attack the given unit, without taking into
    /// account the unit's attack heat. Takes into account only the unit's
    /// attack range, and the location of the unit.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is a healer, or not a robot.
    pub fn can_attack(&self, _robot_id: UnitID, _target_id: UnitID) -> Result<bool, Error> {
        unimplemented!();
    }

    /// Whether the robot is ready to attack. Tests whether the robot's attack
    /// heat is sufficiently low.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is a healer, or not a robot.
    pub fn is_attack_ready(&self, _robot_id: UnitID) -> Result<bool, Error> {
        unimplemented!();
    }

    /// Attacks the robot, dealing the unit's standard amount of damage.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is a healer, or not a robot.
    /// * GameError::InvalidAction - the robot cannot attack that location.
    pub fn attack(&mut self, _robot_id: UnitID, _target_id: UnitID) -> Result<(), Error> {
        unimplemented!();
    }

    // ************************************************************************
    // ************************* RESEARCH METHODS *****************************
    // ************************************************************************

    /// Returns research info of the current player.
    fn get_research(&self) -> ResearchInfo {
        let team = self.team();
        self.get_team_info(team).research.clone()
    }

    /// Returns mutable research info of the current player.
    fn get_research_mut(&mut self) -> &mut ResearchInfo {
        let team = self.team();
        &mut self.get_team_info_mut(team).research
    }

    /// The research info of the current team, including what branch is
    /// currently being researched, the number of rounds left.
    ///
    /// Note that mutating this object by resetting or queueing research
    /// does not have any effect. You must call the mutators on world.
    pub fn research_info(&self) -> ResearchInfo {
        self.get_research()
    }

    /// Resets the research queue to be empty. Returns true if the queue was
    /// not empty before, and false otherwise.
    pub fn reset_research(&mut self) -> bool {
        self.get_research_mut().reset_queue()
    }

    /// Adds a branch to the back of the queue, if it is a valid upgrade, and
    /// starts research if it is the first in the queue.
    ///
    /// Returns whether the branch was successfully added.
    pub fn queue_research(&mut self, branch: &Branch) -> bool {
        self.get_research_mut().add_to_queue(branch)
    }

    /// Update the current research and process any completed upgrades.
    fn process_research(&mut self, team: Team) -> Result<(), Error> {
        if let Some(branch) = self.get_team_info_mut(team).research.next_round()? {
            for (_, unit) in self.units.iter_mut() {
                if unit.unit_type() == branch {
                    unit.research()?;
                }
            }
            Ok(())
        } else {
            Ok(())
        }
    }

    // ************************************************************************
    // *************************** WORKER METHODS *****************************
    // ************************************************************************

    /// Whether the worker is ready to harvest. The worker cannot already have
    /// performed an action this round.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a worker.
    pub fn can_harvest(&self, _worker_id: UnitID) -> Result<bool, Error> {
        unimplemented!();
    }

    /// Harvests up to the worker's harvest amount of karbonite from the given
    /// location, adding it to the team's resource pool.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a worker.
    /// * GameError::InvalidLocation - the location is off the map.
    /// * GameError::InvalidAction - the worker is not ready to harvest.
    pub fn harvest(&mut self, _worker_id: UnitID, _direction: Direction)
                   -> Result<(), Error> {
        unimplemented!();
    }

    /// Whether the worker can blueprint a unit of the given type. The worker
    /// can only blueprint factories, and rockets if Rocketry has been
    /// researched. The team must have sufficient karbonite in its resource
    /// pool. The worker cannot already have performed an action this round.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a worker, or the
    ///   unit type is not a factory or rocket.
    pub fn can_blueprint(&self, _worker_id: UnitID, _unit_type: UnitType)
                         -> Result<bool, Error> {
        unimplemented!();
    }

    /// Blueprints a unit of the given type in the given direction. Subtract
    /// cost of that unit from the team's resource pool.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a worker, or the
    ///   unit type is not a factory or rocket.
    /// * GameError::InvalidLocation - the location is off the map.
    /// * GameError::InvalidAction - the worker is not ready to blueprint.
    pub fn blueprint(&mut self, _worker_id: UnitID, _unit_type: UnitType,
                     _direction: Direction) -> Result<(), Error> {
        unimplemented!();
    }

    /// Whether the worker can build a blueprint with the given ID. The worker
    /// and the blueprint must be adjacent to each other. The worker cannot
    /// already have performed an action this round.
    ///
    /// * GameError::NoSuchUnit - a unit does not exist.
    /// * GameError::TeamNotAllowed - a unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the worker or blueprint is the wrong
    ///   type. A unit that has already been built is no longer a blueprint.
    pub fn can_build(&self, _worker_id: UnitID, _blueprint_id: UnitID)
                     -> Result<bool, Error> {
        unimplemented!();
    }

    /// Blueprints a unit of the given type in the given direction. Subtract
    /// cost of that unit from the team's resource pool.
    ///
    /// * GameError::NoSuchUnit - a unit does not exist.
    /// * GameError::TeamNotAllowed - a unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit or blueprint is the wrong type.
    /// * GameError::InvalidAction - the worker cannot build the blueprint.
    pub fn build(&mut self, _worker_id: UnitID, _blueprint_id: UnitID)
                 -> Result<(), Error> {
        unimplemented!();
    }

    /// Whether the worker is ready to replicate. Tests that the worker's
    /// ability heat is sufficiently low, and that the team has sufficient
    /// karbonite in its resource pool.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a worker.
    pub fn can_replicate(&self, _worker_id: UnitID) -> Result<bool, Error> {
        unimplemented!();
    }

    /// Replicates a worker in the given direction. Subtracts the cost of the
    /// worker from the team's resource pool.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a worker.
    /// * GameError::InvalidLocation - the location is off the map.
    /// * GameError::InvalidAction - the worker is not ready to replicate.
    pub fn replicate(&mut self, _worker_id: UnitID, _direction: Direction)
                     -> Result<(), Error> {
        unimplemented!();
    }

    // ************************************************************************
    // *************************** KNIGHT METHODS *****************************
    // ************************************************************************

    /// Whether the knight can javelin the given robot, without taking into
    /// account the knight's ability heat. Takes into account only the knight's
    /// ability range, and the location of the robot.
    ///
    /// * GameError::InvalidResearchLevel - the ability has not been researched.
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a knight.
    pub fn can_javelin(&self, _knight_id: UnitID, _target_id: UnitID) -> Result<bool, Error> {
        unimplemented!();
    }

    /// Whether the knight is ready to javelin. Tests whether the knight's
    /// ability heat is sufficiently low.
    ///
    /// * GameError::InvalidResearchLevel - the ability has not been researched.
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a knight.
    pub fn is_javelin_ready(&self, _knight_id: UnitID) -> Result<bool, Error> {
        unimplemented!();
    }

    /// Javelins the robot, dealing the amount of ability damage.
    ///
    /// * GameError::InvalidResearchLevel - the ability has not been researched.
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a knight.
    /// * GameError::InvalidAction - the knight cannot javelin that unit.
    pub fn javelin(&mut self, _knight_id: UnitID, _target_id: UnitID) -> Result<(), Error> {
        unimplemented!();
    }

    // ************************************************************************
    // *************************** RANGER METHODS *****************************
    // ************************************************************************

    /// Begins the countdown to snipe a given location. Maximizes the units
    /// attack and movement heats until the ranger has sniped. The ranger may
    /// begin the countdown at any time, including resetting the countdown
    /// to snipe a different location.
    ///
    /// * GameError::InvalidResearchLevel - the ability has not been researched.
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a ranger.
    /// * GameError::InvalidLocation - the location is off the map or on a different planet.
    pub fn begin_snipe(&mut self, _ranger_id: UnitID, _location: MapLocation)
                       -> Result<(), Error> {
        unimplemented!();
    }

    /// If a ranger's snipe has reached the end of its countdown, fires
    /// the shot and resets the attack and movement heats.
    fn _process_ranger(&mut self) {
        unimplemented!();
    }

    // ************************************************************************
    // **************************** MAGE METHODS ******************************
    // ************************************************************************
    
    /// Whether the mage can blink to the given location, without taking into
    /// account the mage's ability heat. Takes into account only the mage's
    /// ability range, the map terrain, positions of other units, and the edge
    /// of the game map.
    ///
    /// * GameError::InvalidResearchLevel - the ability has not been researched.
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a mage.
    /// * GameError::InvalidLocation - the location is outside the vision range.
    pub fn can_blink(&self, _mage_id: UnitID, _location: MapLocation) -> Result<bool, Error> {
        unimplemented!();
    }

    /// Whether the mage is ready to blink. Tests whether the mage's ability
    /// heat is sufficiently low.
    ///
    /// * GameError::InvalidResearchLevel - the ability has not been researched.
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a mage.
    pub fn is_blink_ready(&self, _mage_id: UnitID) -> Result<bool, Error> {
        unimplemented!();
    }

    /// Blinks the mage to the given location.
    ///
    /// * GameError::InvalidResearchLevel - the ability has not been researched.
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a mage.
    /// * GameError::InvalidAction - the mage cannot blink to that location.
    pub fn blink(&mut self, _mage_id: UnitID, _location: MapLocation) -> Result<(), Error> {
        unimplemented!();
    }

    // ************************************************************************
    // *************************** HEALER METHODS *****************************
    // ************************************************************************

    /// Whether the healer can heal the given robot, without taking into
    /// account the healer's attack heat. Takes into account only the healer's
    /// attack range, and the location of the robot.
    ///
    /// * GameError::NoSuchUnit - a unit does not exist.
    /// * GameError::TeamNotAllowed - the first unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the healer or robot is not the right type.
    pub fn can_heal(&self, _healer_id: UnitID, _robot_id: UnitID) -> Result<bool, Error> {
        unimplemented!();
    }

    /// Whether the healer is ready to heal. Tests whether the healer's attack
    /// heat is sufficiently low.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a healer.
    pub fn is_heal_ready(&self, _healer_id: UnitID) -> Result<bool, Error> {
        unimplemented!();
    }

    /// Heals the robot, dealing the healer's standard amount of "damage".
    ///
    /// * GameError::NoSuchUnit - a unit does not exist.
    /// * GameError::TeamNotAllowed - the first unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the healer or robot is not the right type.
    /// * GameError::InvalidAction - the healer cannot heal that unit.
    pub fn heal(&mut self, _healer_id: UnitID, _robot_id: UnitID) -> Result<(), Error> {
        unimplemented!();
    }

    /// Whether the healer can overcharge the given robot, without taking into
    /// account the healer's ability heat. Takes into account only the healer's
    /// ability range, and the location of the robot.
    ///
    /// * GameError::InvalidResearchLevel - the ability has not been researched.
    /// * GameError::NoSuchUnit - a unit does not exist.
    /// * GameError::TeamNotAllowed - the first unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the healer or robot is not the right type.
    pub fn can_overcharge(&self, _healer_id: UnitID, _robot_id: UnitID)
                          -> Result<bool, Error> {
        unimplemented!();
    }

    /// Whether the healer is ready to overcharge. Tests whether the healer's
    /// ability heat is sufficiently low.
    ///
    /// * GameError::InvalidResearchLevel - the ability has not been researched.
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a healer.
    pub fn is_overcharge_ready(&self, _healer_id: UnitID) -> Result<bool, Error> {
        unimplemented!();
    }

    /// Overcharges the robot, resetting the robot's cooldowns.
    ///
    /// * GameError::InvalidResearchLevel - the ability has not been researched.
    /// * GameError::NoSuchUnit - a unit does not exist.
    /// * GameError::TeamNotAllowed - the first unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the healer or robot is not the right type.
    /// * GameError::InvalidAction - the healer cannot overcharge that unit.
    pub fn overcharge(&mut self, _healer_id: UnitID, _robot_id: UnitID)
                      -> Result<(), Error> {
        unimplemented!();
    }

    // ************************************************************************
    // ************************** FACTORY METHODS *****************************
    // ************************************************************************

    /// Adds a unit to the factory's production queue. Does nothing if the
    /// production queue is full. Returns whether the unit was added.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist.
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a factory, or the
    ///   queued unit type is not a robot.
    pub fn queue_robot(&mut self, _factory_id: UnitID, _unit_type: UnitType)
                       -> Result<bool, Error> {
        unimplemented!();
    }

    /// Tests whether the factory is able to degarrison a unit in the given
    /// direction. There must be space in that direction.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a factory.
    /// * GameError::InvalidLocation - the location is off the map.
    pub fn can_degarrison_factory(&self, _factory_id: UnitID,
                                  _direction: Direction) -> Result<bool, Error> {
        unimplemented!();
    }

    /// Degarrisons a robot from the garrison of the specified factory. Robots
    /// are degarrisoned in the order they garrisoned.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a factory.
    /// * GameError::InvalidLocation - the location is off the map.
    /// * GameError::InvalidAction - the factory cannot degarrison a unit.
    pub fn degarrison_factory(&mut self, _factory_id: UnitID,
                              _direction: Direction) -> Result<(), Error> {
        unimplemented!();
    }

    /// Process the end of the turn for factories. If a factory added a unit
    /// to its garrison, also mark that unit down in the game world.
    fn _process_factory(&self) {
        unimplemented!()
    }

    // ************************************************************************
    // *************************** ROCKET METHODS *****************************
    // ************************************************************************

    /// Whether the robot can garrison inside the rocket. The robot must be
    /// ready to move and adjacent to the rocket. The rocket must be on the
    /// same team and have enough space.
    ///
    /// * GameError::NoSuchUnit - a unit does not exist.
    /// * GameError::TeamNotAllowed - the rocket is not on the current player's team.
    /// * GameError::InappropriateUnitType - the robot or rocket are the wrong type.
    pub fn can_garrison_rocket(&self, rocket_id: UnitID, robot_id: UnitID)
                        -> Result<bool, Error> {
        let robot = self.get_unit(robot_id)?;
        let rocket = self.get_unit(rocket_id)?;
        rocket.can_garrison(robot)
    }

    /// Moves the robot into the garrison of the rocket.
    ///
    /// * GameError::NoSuchUnit - a unit does not exist.
    /// * GameError::TeamNotAllowed - the robot or rocket is not on the current player's team.
    /// * GameError::InappropriateUnitType - the robot or rocket are the wrong type.
    /// * GameError::InvalidAction - the robot cannot garrison inside the rocket.
    pub fn garrison_rocket(&mut self, rocket_id: UnitID, robot_id: UnitID)
                    -> Result<(), Error> {
        if self.can_garrison_rocket(rocket_id, robot_id)? {
            self.remove_unit(robot_id);
            self.get_unit_mut(rocket_id)?.garrison(robot_id)?;
            self.get_unit_mut(robot_id)?.board_rocket(rocket_id)?;
            Ok(())
        } else {
            Err(GameError::InvalidAction)?
        }
    }

    /// Tests whether the given rocket is able to degarrison a unit in the
    /// given direction. There must be space in that direction, and the unit
    /// must be ready to move.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a rocket.
    /// * GameError::InvalidLocation - the location is off the map.
    pub fn can_degarrison_rocket(&self, rocket_id: UnitID, direction: Direction)
                                 -> Result<bool, Error> {
        let rocket = self.get_unit(rocket_id)?;
        if rocket.can_degarrison_unit()? {
            let robot = self.get_unit(rocket.garrisoned_units()?[0])?;
            let loc = rocket.location().map_location()?.add(direction);
            Ok(self.is_occupiable(loc)? && robot.is_move_ready()?)
        } else {
            Ok(false)
        }
    }

    /// Degarrisons a robot from the garrison of the specified rocket. Robots
    /// are degarrisoned in the order they garrisoned.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a rocket.
    /// * GameError::InvalidLocation - the location is off the map.
    /// * GameError::InvalidAction - the rocket cannot degarrison a unit.
    pub fn degarrison_rocket(&mut self, rocket_id: UnitID, direction: Direction)
                      -> Result<(), Error> {
        if self.can_degarrison_rocket(rocket_id, direction)? {
            let (robot_id, rocket_loc) = {
                let rocket = self.get_unit_mut(rocket_id)?;
                (rocket.degarrison_unit()?, rocket.location().map_location()?)
            };
            let robot_loc = rocket_loc.add(direction);
            self.get_unit_mut(robot_id)?.move_to(robot_loc)?;
            self.place_unit(robot_id);
            Ok(())
        } else {
            Err(GameError::InvalidAction)?
        }
    }

    /// Whether the rocket can launch into space. The rocket can launch if the
    /// it has never been used before.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a rocket.
    pub fn can_launch_rocket(&mut self, rocket_id: UnitID, destination: MapLocation)
                             -> Result<bool, Error> {
        let rocket = self.get_unit(rocket_id)?;
        if rocket.is_rocket_used()? {
            return Ok(false);
        }
        let map = &self.get_planet_info(destination.planet).map;
        Ok(map.on_map(destination) && map.is_passable_terrain_at(destination)?)
    }

    /// Launches the rocket into space. If the destination is not on the map of
    /// the other planet, the rocket flies off, never to be seen again.
    ///
    /// * GameError::NoSuchUnit - the unit does not exist (inside the vision range).
    /// * GameError::TeamNotAllowed - the unit is not on the current player's team.
    /// * GameError::InappropriateUnitType - the unit is not a rocket.
    /// * GameError::InvalidAction - the rocket cannot launch.
    pub fn launch_rocket(&mut self, rocket_id: UnitID, destination: MapLocation)
                         -> Result<(), Error> {
        if self.can_launch_rocket(rocket_id, destination)? {
            let takeoff_loc = self.get_unit(rocket_id)?.location().map_location()?;
            self.get_unit_mut(rocket_id)?.launch_rocket()?;
            let landing_round = self.round + self.orbit.duration(self.round);
            if self.rocket_landings.contains_key(&landing_round) {
                self.rocket_landings.get_mut(&landing_round).unwrap().push((rocket_id, destination));
            } else {
                self.rocket_landings.insert(landing_round, vec![(rocket_id, destination)]);
            }
            for dir in Direction::all() {
                self.damage_location(takeoff_loc.add(dir), ROCKET_BLAST_DAMAGE)?;
            }
            Ok(())
        } else {
            Err(GameError::InvalidAction)?
        }
    }

    /// Lands the rocket, damaging the units in adjacent squares. The rocket
    /// is destroyed if it lands on a factory, rocket, or impassable terrain.
    fn land_rocket(&mut self, id: UnitID, destination: MapLocation)
                   -> Result<(), Error> {
        if self.units_by_loc.contains_key(&destination) {
            let victim_id = *self.units_by_loc.get(&destination).unwrap();
            let should_destroy_rocket = match self.get_unit(victim_id)?.unit_type() {
                UnitType::Rocket => true,
                UnitType::Factory => true,
                _ => false,
            };
            if should_destroy_rocket {
                self.destroy_unit(id)?;
            }
            self.destroy_unit(victim_id)?;
        } else {
            self.get_unit_mut(id)?.land_rocket(destination)?;
            self.place_unit(id);
        }

        for dir in Direction::all() {
            self.damage_location(destination.add(dir), ROCKET_BLAST_DAMAGE)?;
        }

        Ok(())
    }

    fn process_rockets(&mut self) -> Result<(), Error> {
        let landings = if let Some(landings) = self.rocket_landings.get(&self.round) {
            landings.clone()
        } else {
            vec![]
        };
        for &(id, location) in landings.iter() {
            self.land_rocket(id, location)?;
        }
        Ok(())
    }

    // ************************************************************************
    // ****************************** GAME LOOP *******************************
    // ************************************************************************

    /// Updates the current player in the game. If a round of four turns has
    /// finished, also processes the end of the round. This includes updating
    /// unit cooldowns, rocket landings, asteroid strikes, research, etc.
    ///
    /// * GameError::InternalEngineError - something happened here...
    pub fn next_turn(&mut self) -> Result<(), Error> {
        self.player_to_move = match self.player_to_move {
            Player { team: Team::Red, planet: Planet::Earth } => Player { team: Team::Blue, planet: Planet::Earth},
            Player { team: Team::Blue, planet: Planet::Earth } => Player { team: Team::Red, planet: Planet::Mars},
            Player { team: Team::Red, planet: Planet::Mars } => Player { team: Team::Blue, planet: Planet::Mars},
            Player { team: Team::Blue, planet: Planet::Mars } => {
                // This is the last player to move, so we can advance to the next round.
                self.next_round()?;
                Player { team: Team::Red, planet: Planet::Earth}
            },
        };
        Ok(())
    }

    fn next_round(&mut self) -> Result<(), Error> {
        self.round += 1;

        // Update unit cooldowns.
        for unit in &mut self.units.values_mut() {
            unit.next_round();
        }

        // Land rockets.
        self.process_rockets()?;

        // Process any potential asteroid impacts.
        self.process_asteroids();

        // Update the current research and process any completed upgrades.
        self.process_research(Team::Red)?;
        self.process_research(Team::Blue)?;

        Ok(())
    }

    pub fn apply(&mut self, delta: Delta) -> Result<(), Error> {
        match delta {
            Delta::EndTurn => self.next_turn(),
            Delta::Move{id, direction} => self.move_robot(id, direction),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unit_create() {
        // Create the game world, and create and add some robots.
        let mut world = GameWorld::new(GameMap::test_map()).expect("invalid test map");
        let loc_a = MapLocation::new(Planet::Earth, 0, 1);
        let loc_b = MapLocation::new(Planet::Earth, 0, 2);
        let id_a = world.create_unit(Team::Red, loc_a, UnitType::Knight).unwrap();
        let id_b = world.create_unit(Team::Red, loc_b, UnitType::Knight).unwrap();

        // The robots have different IDs.
        assert_ne!(id_a, id_b);

        // Both robots exist and are at the right locations.
        let unit_a = world.get_unit(id_a).unwrap();
        let unit_b = world.get_unit(id_b).unwrap();
        assert_eq!(unit_a.location(), OnMap(loc_a));
        assert_eq!(unit_b.location(), OnMap(loc_b));
    }

    #[test]
    fn test_unit_move() {
        // Create the game world.
        let mut world = GameWorld::new(GameMap::test_map()).expect("invalid test map");
        let loc_a = MapLocation::new(Planet::Earth, 5, 5);
        let loc_b = MapLocation::new(Planet::Earth, 6, 5);

        // Create and add some robots. B is one square east of A.
        let a = world.create_unit(Team::Red, loc_a, UnitType::Knight).unwrap();
        let b = world.create_unit(Team::Red, loc_b, UnitType::Knight).unwrap();

        // Robot A cannot move east, as this is where B is. However,
        // it can move northeast.
        assert![!world.can_move(a, Direction::East).unwrap()];
        assert![world.can_move(a, Direction::Northeast).unwrap()];
        world.move_robot(a, Direction::Northeast).unwrap();

        // A is now one square north of B. B cannot move north to
        // A's new location, but can move west to A's old location.
        assert![!world.can_move(b, Direction::North).unwrap()];
        assert![world.can_move(b, Direction::West).unwrap()];
        world.move_robot(b, Direction::West).unwrap();

        // A cannot move again until its cooldowns are reset.
        assert![!world.can_move(a, Direction::South).unwrap()];
        assert![world.move_robot(a, Direction::South).is_err()];
        assert![world.next_round().is_ok()];

        // Finally, let's test that A cannot move back to its old square.
        assert![!world.can_move(a, Direction::Southwest).unwrap()];
        assert![world.can_move(a, Direction::South).unwrap()];
        world.move_robot(a, Direction::South).unwrap();
    }

    #[test]
    fn test_rocket_success() {
        // Create the game world.
        let mut world = GameWorld::new(GameMap::test_map()).expect("invalid test map");
        let earth_loc = MapLocation::new(Planet::Earth, 5, 5);
        let mars_loc = MapLocation::new(Planet::Mars, 5, 5);
        let rocket = world.create_unit(Team::Red, earth_loc, UnitType::Rocket).unwrap();

        // Create units around the target location.
        let mut bystanders: Vec<UnitID> = vec![];
        let mut direction = Direction::North;
        for _ in 0..8 {
            bystanders.push(world.create_unit(Team::Red, earth_loc.add(direction), UnitType::Knight).unwrap());
            bystanders.push(world.create_unit(Team::Red, mars_loc.add(direction), UnitType::Knight).unwrap());
            direction = direction.rotate_right();
        }

        // Launch the rocket, and force land it.
        world.launch_rocket(rocket, mars_loc).unwrap();
        assert_eq![world.get_unit(rocket).unwrap().location(), InSpace];
        world.land_rocket(rocket, mars_loc).unwrap();
        assert_eq![world.get_unit(rocket).unwrap().location(), OnMap(mars_loc)];
        let damaged_knight_health = 200;
        for id in bystanders.iter() {
            assert_eq![world.get_unit(*id).unwrap().health(), damaged_knight_health];
        }
    }

    #[test]
    fn test_rocket_failure() {
        // Create the game world.
        let mut world = GameWorld::new(GameMap::test_map()).expect("invalid test map");
        let earth_loc_a = MapLocation::new(Planet::Earth, 0, 0);
        let earth_loc_b = MapLocation::new(Planet::Earth, 0, 2);
        let mars_loc_off_map = MapLocation::new(Planet::Mars, 10000, 10000);
        let mars_loc_impassable = MapLocation::new(Planet::Mars, 0, 0);
        world.get_planet_info_mut(Planet::Mars).map.is_passable_terrain[0][0] = false;
        let mars_loc_knight = MapLocation::new(Planet::Mars, 0, 1);
        let mars_loc_factory = MapLocation::new(Planet::Mars, 0, 2);
        let rocket_a = world.create_unit(Team::Red, earth_loc_a, UnitType::Rocket).unwrap();
        let rocket_b = world.create_unit(Team::Red, earth_loc_b, UnitType::Rocket).unwrap();
        let knight = world.create_unit(Team::Blue, mars_loc_knight, UnitType::Knight).unwrap();
        let factory = world.create_unit(Team::Blue, mars_loc_factory, UnitType::Factory).unwrap();

        // Failed launches.
        assert![world.launch_rocket(rocket_a, mars_loc_off_map).is_err()];
        assert![world.launch_rocket(rocket_a, mars_loc_impassable).is_err()];

        // Rocket landing on a robot should destroy the robot.
        world.launch_rocket(rocket_a, mars_loc_knight).unwrap();
        world.land_rocket(rocket_a, mars_loc_knight).unwrap();
        assert![world.get_unit(rocket_a).is_ok()];
        assert![world.get_unit(knight).is_err()];

        // Rocket landing on a factory should destroy both units.
        world.launch_rocket(rocket_b, mars_loc_factory).unwrap();
        world.land_rocket(rocket_b, mars_loc_factory).unwrap();
        assert![world.get_unit(rocket_b).is_err()];
        assert![world.get_unit(factory).is_err()];
    }

    #[test]
    fn test_rocket_garrison() {
        // Create the game world and the rocket for this test.
        let mut world = GameWorld::new(GameMap::test_map()).expect("invalid test map");
        let takeoff_loc = MapLocation::new(Planet::Earth, 10, 10);        
        let rocket = world.create_unit(Team::Red, takeoff_loc, UnitType::Rocket).unwrap();

        // Correct garrisoning.
        let valid_boarder = world.create_unit(Team::Red, takeoff_loc.add(Direction::North), UnitType::Knight).unwrap();
        assert![world.garrison_rocket(rocket, valid_boarder).is_ok()];
        assert_eq![world.get_unit(valid_boarder).unwrap().location(), InRocket(rocket)];

        // Boarding fails when too far from the rocket.
        let invalid_boarder_too_far = world.create_unit(Team::Red, takeoff_loc.add(Direction::North).add(Direction::North), UnitType::Knight).unwrap();
        assert![world.garrison_rocket(rocket, invalid_boarder_too_far).is_err()];

        // Boarding fails when the robot has already moved.
        // uncomment when movement cooldown is implemented
        //assert![world.move_robot(invalid_boarder_too_far, Direction::South).is_ok()];
        //let invalid_boarder_already_moved = invalid_boarder_too_far;
        //assert![!world.get_unit(invalid_boarder_already_moved).unwrap().is_move_ready()];
        //assert![world.garrison_rocket(invalid_boarder_already_moved, rocket).is_err()];

        // Factories and rockets cannot board rockets.
        let invalid_boarder_factory = world.create_unit(Team::Red, takeoff_loc.add(Direction::Southeast), UnitType::Factory).unwrap();
        assert![world.garrison_rocket(rocket, invalid_boarder_factory).is_err()];
        let invalid_boarder_rocket = world.create_unit(Team::Red, takeoff_loc.add(Direction::South), UnitType::Rocket).unwrap();
        assert![world.garrison_rocket(rocket, invalid_boarder_rocket).is_err()];

        // Rockets can be loaded up to their capacity...
        for _ in 1..8 {
            let valid_extra_boarder = world.create_unit(Team::Red, takeoff_loc.add(Direction::East), UnitType::Knight).unwrap();
            assert![world.garrison_rocket(rocket, valid_extra_boarder).is_ok()];
        }

        // ... but not beyond their capacity.
        let invalid_boarder_rocket_full = world.create_unit(Team::Red, takeoff_loc.add(Direction::East), UnitType::Knight).unwrap();
        assert![world.garrison_rocket(rocket, invalid_boarder_rocket_full).is_err()];

        // A unit should not be able to board another team's rocket.
        let blue_takeoff_loc = MapLocation::new(Planet::Earth, 5, 5);
        let blue_rocket = world.create_unit(Team::Blue, blue_takeoff_loc, UnitType::Rocket).unwrap();
        let invalid_boarder_wrong_team = world.create_unit(Team::Red, blue_takeoff_loc.add(Direction::North), UnitType::Knight).unwrap();
        assert![world.garrison_rocket(blue_rocket, invalid_boarder_wrong_team).is_err()];
    }

    #[test]
    fn test_rocket_degarrison() {
        // Create the game world and the rocket for this test.
        let mut world = GameWorld::new(GameMap::test_map()).expect("invalid test map");
        let takeoff_loc = MapLocation::new(Planet::Earth, 10, 10);        
        let rocket = world.create_unit(Team::Red, takeoff_loc, UnitType::Rocket).unwrap();
        
        // Load the rocket with robots.
        for _ in 0..2 {
            let robot = world.create_unit(Team::Red, takeoff_loc.add(Direction::North), UnitType::Knight).unwrap();
            assert![world.garrison_rocket(rocket, robot).is_ok()];
        }

        // Fly the rocket to Mars.
        let landing_loc = MapLocation::new(Planet::Mars, 10, 10);
        assert![world.launch_rocket(rocket, landing_loc).is_ok()];
        assert![world.land_rocket(rocket, landing_loc).is_ok()];

        // Cannot degarrison in the same round.
        assert![world.degarrison_rocket(rocket, Direction::North).is_err()];

        // Correct degarrisoning.
        world.next_round().unwrap();
        assert![world.degarrison_rocket(rocket, Direction::North).is_ok()];

        // Cannot degarrison into an occupied square.
        assert![world.degarrison_rocket(rocket, Direction::North).is_err()];

        // Cannot degarrison into an impassable square.
        world.get_planet_info_mut(Planet::Mars).map.is_passable_terrain[10][11] = false;
        assert![world.degarrison_rocket(rocket, Direction::East).is_err()];

        // Correct degarrisoning, again.
        assert![world.degarrison_rocket(rocket, Direction::South).is_ok()];

        // Cannot degarrison an empty rocket.
        assert![world.degarrison_rocket(rocket, Direction::West).is_err()];
    }
}
