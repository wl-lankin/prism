//! The "near you" ranking. Radio Browser has no distance order and few stations carry
//! coordinates, so the list is built from three bands, each in the directory's click order:
//! stations within the radius, stations of the same region, the country's most clicked.

use std::collections::HashSet;

use super::location::{Place, PlaceOrigin};
use super::model::Station;

/// Band 1 reaches this far.
pub const NEAR_RADIUS_KM: f32 = 75.0;
/// The list never holds more.
pub const NEAR_MAX: usize = 200;

/// The three bands in order, each kept in the order it came in (the directory sorts by
/// clicks). A station shows once, at its first place: the same uuid, or the same name and
/// stream. `distance_km` stays for band 1 only. A manual place has no band 1.
pub fn rank(
    place: &Place,
    geo: Vec<Station>,
    region: Vec<Station>,
    country: Vec<Station>,
    max: usize,
) -> Vec<Station> {
    let geo = if place.origin == PlaceOrigin::Manual {
        Vec::new()
    } else {
        geo
    };
    let mut ids: HashSet<String> = HashSet::new();
    let mut streams: HashSet<(String, String)> = HashSet::new();
    let mut ranked = Vec::new();
    let in_radius = |station: &Station| {
        station
            .distance_km
            .is_some_and(|km| km.is_finite() && km <= NEAR_RADIUS_KM)
    };
    let bands = [
        (geo.into_iter().filter(in_radius).collect::<Vec<_>>(), true),
        (region, false),
        (country, false),
    ];
    for (band, keeps_distance) in bands {
        for mut station in band {
            if ranked.len() >= max {
                return ranked;
            }
            let stream = station
                .url_resolved
                .clone()
                .unwrap_or_else(|| station.url.clone());
            let key = (station.name.to_lowercase(), stream);
            if ids.contains(&station.id) || streams.contains(&key) {
                continue;
            }
            ids.insert(station.id.clone());
            streams.insert(key);
            if !keeps_distance {
                station.distance_km = None;
            }
            ranked.push(station);
        }
    }
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn testhausen(origin: PlaceOrigin) -> Place {
        let detected = origin == PlaceOrigin::Detected;
        Place {
            country_code: "DE".into(),
            region: Some("Testland".into()),
            city: detected.then(|| "Testhausen".into()),
            lat: detected.then_some(50.0),
            lon: detected.then_some(10.0),
            origin,
            detected_at: detected.then_some(1),
        }
    }

    fn station(n: u32) -> Station {
        Station {
            id: format!("radio:00000000-0000-4000-8000-{n:012x}"),
            name: format!("Demo Radio {n}"),
            url: format!("http://stream.example.org/stream/{n}"),
            url_resolved: Some(format!("http://stream.example.org/stream/{n}")),
            homepage: None,
            favicon: None,
            cover_id: None,
            country_code: Some("DE".into()),
            region: Some("Testland".into()),
            language: None,
            tags: Vec::new(),
            codec: Some("MP3".into()),
            bitrate_kbps: Some(128),
            hls: false,
            last_check_ok: true,
            distance_km: None,
        }
    }

    fn near(n: u32, km: f32) -> Station {
        Station {
            distance_km: Some(km),
            ..station(n)
        }
    }

    fn numbers(list: &[Station]) -> Vec<u32> {
        list.iter()
            .map(|s| u32::from_str_radix(&s.id[s.id.len() - 12..], 16).unwrap())
            .collect()
    }

    #[test]
    fn near_bands_in_order() {
        let place = testhausen(PlaceOrigin::Detected);
        let ranked = rank(
            &place,
            vec![near(2, 12.0), near(1, 40.0)],
            vec![station(5), station(3), station(4)],
            vec![station(10), station(6), station(9), station(7), station(8)],
            NEAR_MAX,
        );
        assert_eq!(numbers(&ranked), vec![2, 1, 5, 3, 4, 10, 6, 9, 7, 8]);
        assert_eq!(ranked[0].distance_km, Some(12.0));
        assert_eq!(ranked[1].distance_km, Some(40.0));
        assert!(ranked[2..].iter().all(|s| s.distance_km.is_none()));
    }

    #[test]
    fn near_dedupes_across_bands() {
        let place = testhausen(PlaceOrigin::Detected);
        // Station 7 under another uuid: the same name and stream.
        let twin = Station {
            id: station(70).id,
            ..station(7)
        };
        // A region-band station that carries a distance does not keep it.
        let far = near(4, 30.0);
        let ranked = rank(
            &place,
            vec![near(1, 5.0), near(7, 20.0)],
            vec![station(1), far, twin.clone(), station(3)],
            vec![station(3), station(7), station(5), twin],
            NEAR_MAX,
        );
        assert_eq!(numbers(&ranked), vec![1, 7, 4, 3, 5]);
        assert_eq!(ranked[2].distance_km, None);
    }

    #[test]
    fn geo_band_beyond_the_radius_is_dropped() {
        let place = testhausen(PlaceOrigin::Detected);
        let ranked = rank(
            &place,
            vec![near(1, 80.0), near(2, 75.0)],
            vec![],
            vec![],
            10,
        );
        assert_eq!(numbers(&ranked), vec![2]);
    }

    #[test]
    fn manual_place_skips_the_geo_band_in_ranking() {
        let place = testhausen(PlaceOrigin::Manual);
        let ranked = rank(
            &place,
            vec![near(1, 5.0)],
            vec![station(2)],
            vec![station(3)],
            10,
        );
        assert_eq!(numbers(&ranked), vec![2, 3]);
    }

    #[test]
    fn the_list_is_capped() {
        let place = testhausen(PlaceOrigin::Detected);
        let country: Vec<Station> = (1..=500).map(station).collect();
        let ranked = rank(&place, vec![], vec![], country, NEAR_MAX);
        assert_eq!(ranked.len(), NEAR_MAX);
        assert_eq!(numbers(&ranked)[..3], [1, 2, 3]);
    }
}
