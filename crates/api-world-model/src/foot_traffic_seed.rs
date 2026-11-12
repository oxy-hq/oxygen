//! Compile-time seed for the in-memory foot-traffic caches.
//!
//! Embedded as Rust source (not a runtime JSON file) so a distributed
//! binary ships with warm BestTime data and never touches the filesystem.
//! Parsed once on first access into the process-lifetime in-memory maps;
//! live BestTime fetches update those maps but are never persisted.
//!
//! Regenerate from a captured snapshot if the seed ever needs refreshing.

pub(crate) const SEED_JSON: &str = r##"{
  "live": {
    "74be4039-8918-421e-a2d3-cdb21df38195": {
      "at_unix": 1779890922,
      "value": {
        "key": "74be4039-8918-421e-a2d3-cdb21df38195",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    },
    "c129c966-4dd6-45ea-9ba0-ab43bf10e7c0": {
      "at_unix": 1779890925,
      "value": {
        "key": "c129c966-4dd6-45ea-9ba0-ab43bf10e7c0",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    },
    "ff7e046e-bec0-4939-8985-66c5aa52b127": {
      "at_unix": 1779890925,
      "value": {
        "key": "ff7e046e-bec0-4939-8985-66c5aa52b127",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    },
    "b7f491d7-67d6-45d5-a4f3-009670a74899": {
      "at_unix": 1779890921,
      "value": {
        "key": "b7f491d7-67d6-45d5-a4f3-009670a74899",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    },
    "83248b04-7519-487a-90f6-eb62ee45caf6": {
      "at_unix": 1779890920,
      "value": {
        "key": "83248b04-7519-487a-90f6-eb62ee45caf6",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    },
    "1632f6e7-8bf7-4e42-9f14-7fefca1b2572": {
      "at_unix": 1779890920,
      "value": {
        "key": "1632f6e7-8bf7-4e42-9f14-7fefca1b2572",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    },
    "bf47a19b-b462-40c3-972d-4738a3c35ba5": {
      "at_unix": 1779890925,
      "value": {
        "key": "bf47a19b-b462-40c3-972d-4738a3c35ba5",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    },
    "f78978f5-c0a7-49c4-b6ef-f7bfa6faa50a": {
      "at_unix": 1779890925,
      "value": {
        "key": "f78978f5-c0a7-49c4-b6ef-f7bfa6faa50a",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    },
    "ed8ea8a1-474a-41d3-a06a-c8dde982ede9": {
      "at_unix": 1779890925,
      "value": {
        "key": "ed8ea8a1-474a-41d3-a06a-c8dde982ede9",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    },
    "0f109e7e-16ac-4f26-91a3-e398421fb65d": {
      "at_unix": 1779890921,
      "value": {
        "key": "0f109e7e-16ac-4f26-91a3-e398421fb65d",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    },
    "e814f363-a74e-40e8-9c1c-9201ccd09841": {
      "at_unix": 1779890920,
      "value": {
        "key": "e814f363-a74e-40e8-9c1c-9201ccd09841",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    },
    "644f162a-5077-4a68-a214-424275b106e2": {
      "at_unix": 1779890925,
      "value": {
        "key": "644f162a-5077-4a68-a214-424275b106e2",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    },
    "311f84a7-9681-48a0-b6e1-b1977fa6422d": {
      "at_unix": 1779890920,
      "value": {
        "key": "311f84a7-9681-48a0-b6e1-b1977fa6422d",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    },
    "d31c147e-d44e-4864-a40c-cc8d9307e410": {
      "at_unix": 1779890925,
      "value": {
        "key": "d31c147e-d44e-4864-a40c-cc8d9307e410",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    },
    "00cfb6b1-ff06-4208-8065-73d2ac19094c": {
      "at_unix": 1779890925,
      "value": {
        "key": "00cfb6b1-ff06-4208-8065-73d2ac19094c",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    },
    "08900d15-6e50-4c42-be87-c4c646f9ea47": {
      "at_unix": 1779890925,
      "value": {
        "key": "08900d15-6e50-4c42-be87-c4c646f9ea47",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    },
    "86b65fc8-2488-456f-9065-5954e9e78414": {
      "at_unix": 1779890932,
      "value": {
        "key": "86b65fc8-2488-456f-9065-5954e9e78414",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    },
    "4219821a-daf1-4fbf-ba21-c625de0bd398": {
      "at_unix": 1779890920,
      "value": {
        "key": "4219821a-daf1-4fbf-ba21-c625de0bd398",
        "live_busyness": 0.0,
        "forecast_busyness": 0.0,
        "delta": 0.0,
        "venue_open": false
      }
    }
  },
  "radar": {
    "37.30,-121.95,15000": {
      "at_unix": 1779891000,
      "venues": [
        {
          "venue_id": "ven_416966394253422d6535765241346a77694b58512d56354a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.420941,
          "lon": -122.0933529,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_45777057304232674a43625241346a4f7673696b6f39534a496843",
          "venue_name": "Dave & Buster's Milpitas - San Jose",
          "lat": 37.417728,
          "lon": -121.8979516,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_554367754e4533444a72315241346a3232567241412d534a496843",
          "venue_name": "Dishdash Middle Eastern Cuisine",
          "lat": 37.376177,
          "lon": -122.030136,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_3842657672563075376d355241346a336953506a39646e4a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.3804068,
          "lon": -122.0740341,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_38555952786a41356c6b685241346a31693536496a5f794a496843",
          "venue_name": "Benihana - Cupertino",
          "lat": 37.3275364,
          "lon": -122.0137843,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_49475f2d626f30453165705241346a4c37784e6c4957574a496843",
          "venue_name": "Din Tai Fung 鼎泰豐",
          "lat": 37.3262207,
          "lon": -121.9441336,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_41444145357567514538375241346a33537a64445454344a496843",
          "venue_name": "Crepevine Restaurants",
          "lat": 37.3925221,
          "lon": -122.0800081,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6b466a536a634e337674675263344b787557566d7953704a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.3300865,
          "lon": -121.8113184,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_386632706354494b6731575241346a4c6a47576859544f4a496843",
          "venue_name": "LUNA Mexican Kitchen - San Jose",
          "lat": 37.3339808,
          "lon": -121.9152907,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_73542d46703567556141635241346a3357466a506e38514a496843",
          "venue_name": "Zareen's",
          "lat": 37.4162147,
          "lon": -122.0795314,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    },
    "37.76,-122.25,15000": {
      "at_unix": 1779891003,
      "venues": [
        {
          "venue_id": "ven_635333316b6c666841644852415968414f2d44574454754a496843",
          "venue_name": "Boudin Bakery",
          "lat": 37.808506,
          "lon": -122.41492,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_3849394e65386a4a47637552415968414f6530446c55634a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.8077951,
          "lon": -122.4185119,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_38377338374e4e487550515241596841477645634858484a496843",
          "venue_name": "Tony's Pizza Napoletana",
          "lat": 37.8003815,
          "lon": -122.4090457,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6f6c512d6b5876696674705241596841795f697a4753484a496843",
          "venue_name": "Fog Harbor Fish House",
          "lat": 37.8089961,
          "lon": -122.4102878,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_304a6947337a6b544743395241596841795f697a4753484a496843",
          "venue_name": "Pier Market Seafood Restaurant",
          "lat": 37.8097776,
          "lon": -122.4105536,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6f4137773034677262367352415968417950397870385f4a496843",
          "venue_name": "Bubba Gump Shrimp Co.",
          "lat": 37.8111257,
          "lon": -122.4103593,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_45435f707a5735425265645241346a2d5a5531425954724a496843",
          "venue_name": "La Taqueria",
          "lat": 37.7508961,
          "lon": -122.4180867,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6335614b486d414e774d4e5241596841795f68655963724a496843",
          "venue_name": "Scoma's Restaurant",
          "lat": 37.808932,
          "lon": -122.4184329,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_774235764e7178555f4f6f52415968416934597467564c4a496843",
          "venue_name": "Super Duper Burgers",
          "lat": 37.7868541,
          "lon": -122.4039683,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6b4d4e376f63593649786e524159684147656e426561464a496843",
          "venue_name": "Cioppino's",
          "lat": 37.8080391,
          "lon": -122.4193399,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    },
    "37.39,-121.88,15000": {
      "at_unix": 1779890976,
      "venues": [
        {
          "venue_id": "ven_45777057304232674a43625241346a4f7673696b6f39534a496843",
          "venue_name": "Dave & Buster's Milpitas - San Jose",
          "lat": 37.417728,
          "lon": -121.8979516,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_554367754e4533444a72315241346a3232567241412d534a496843",
          "venue_name": "Dishdash Middle Eastern Cuisine",
          "lat": 37.376177,
          "lon": -122.030136,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_38555952786a41356c6b685241346a31693536496a5f794a496843",
          "venue_name": "Benihana - Cupertino",
          "lat": 37.3275364,
          "lon": -122.0137843,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_49475f2d626f30453165705241346a4c37784e6c4957574a496843",
          "venue_name": "Din Tai Fung 鼎泰豐",
          "lat": 37.3262207,
          "lon": -121.9441336,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6b466a536a634e337674675263344b787557566d7953704a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.3300865,
          "lon": -121.8113184,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_386632706354494b6731575241346a4c6a47576859544f4a496843",
          "venue_name": "LUNA Mexican Kitchen - San Jose",
          "lat": 37.3339808,
          "lon": -121.9152907,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6359467478574b7135745352416f6a7a637a4e324b64664a496843",
          "venue_name": "Denny's Restaurant",
          "lat": 37.3168244,
          "lon": -121.8738022,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6f7139453234736d644b4f5241346a4f583938415762694a496843",
          "venue_name": "El Torito",
          "lat": 37.4331187,
          "lon": -121.897477,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_51726879395468445867615241346a32796c53676452444a496843",
          "venue_name": "Inchin's Bamboo Garden",
          "lat": 37.3762305,
          "lon": -122.0310557,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_493656694a6c7535456d645241346a4c5f4e6f4e52345a4a496843",
          "venue_name": "Dish N Dash",
          "lat": 37.3843966,
          "lon": -121.9272943,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    },
    "37.38,-122.08,15000": {
      "at_unix": 1779891000,
      "venues": [
        {
          "venue_id": "ven_416966394253422d6535765241346a77694b58512d56354a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.420941,
          "lon": -122.0933529,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_554367754e4533444a72315241346a3232567241412d534a496843",
          "venue_name": "Dishdash Middle Eastern Cuisine",
          "lat": 37.376177,
          "lon": -122.030136,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_45557143447056425855635241346a36572d6c503978634a496843",
          "venue_name": "Zareen's Palo Alto",
          "lat": 37.4267457,
          "lon": -122.1440822,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_3842657672563075376d355241346a336953506a39646e4a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.3804068,
          "lon": -122.0740341,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_38555952786a41356c6b685241346a31693536496a5f794a496843",
          "venue_name": "Benihana - Cupertino",
          "lat": 37.3275364,
          "lon": -122.0137843,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_49475f2d626f30453165705241346a4c37784e6c4957574a496843",
          "venue_name": "Din Tai Fung 鼎泰豐",
          "lat": 37.3262207,
          "lon": -121.9441336,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_41444145357567514538375241346a33537a64445454344a496843",
          "venue_name": "Crepevine Restaurants",
          "lat": 37.3925221,
          "lon": -122.0800081,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_386632706354494b6731575241346a4c6a47576859544f4a496843",
          "venue_name": "LUNA Mexican Kitchen - San Jose",
          "lat": 37.3339808,
          "lon": -121.9152907,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_73542d46703567556141635241346a3357466a506e38514a496843",
          "venue_name": "Zareen's",
          "lat": 37.4162147,
          "lon": -122.0795314,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_384247334a3852504a52395241346a3353544a5f4a52734a496843",
          "venue_name": "Oren's Hummus",
          "lat": 37.3947415,
          "lon": -122.0786015,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    },
    "36.58,-121.90,15000": {
      "at_unix": 1779891000,
      "venues": [
        {
          "venue_id": "ven_67553146432d52345a692d5241596a6b6a425a587553594a496843",
          "venue_name": "Crab House",
          "lat": 36.6047202,
          "lon": -121.8924424,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_4d2d38514e66344b4a55345241596a6b6a525a655438424a496843",
          "venue_name": "Old Fisherman’s Grotto",
          "lat": 36.6043278,
          "lon": -121.8928631,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_3439784e5355785675796b5241596a6b486738704a38614a496843",
          "venue_name": "Bubba Gump Shrimp Co.",
          "lat": 36.6167558,
          "lon": -121.9001349,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6b6549596b78507a52644d5241596a6b486769473337564a496843",
          "venue_name": "Fish Hopper",
          "lat": 36.616497,
          "lon": -121.8995541,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_4536487173647a52396a715241596a6b6a56553143374e4a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 36.6056976,
          "lon": -121.858253,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6774316e7175626a7157645241596a684844513554374c4a496843",
          "venue_name": "Monterey's Fish House",
          "lat": 36.6021093,
          "lon": -121.865788,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_41355477346d2d7746456d5241596a6d6e3633704551474a496843",
          "venue_name": "Hula's Island Grill",
          "lat": 36.6139907,
          "lon": -121.9019031,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_4574715136422d765f71705241596a6d586844413754594a496843",
          "venue_name": "La Bicyclette Restaurant",
          "lat": 36.5538698,
          "lon": -121.9227897,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_49646a59356d5542662d575241596a6b5879615556566c4a496843",
          "venue_name": "Denny's Restaurant",
          "lat": 36.5949497,
          "lon": -121.8929484,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_493536693558704a4f72515241596a6b4c675a663033714a496843",
          "venue_name": "Sea Harvest Restaurant & Fish Market",
          "lat": 36.6140028,
          "lon": -121.9005806,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    },
    "37.93,-122.06,15000": {
      "at_unix": 1779891001,
      "venues": [
        {
          "venue_id": "ven_5148733333785f41757952524159686d567036796639484a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.976062,
          "lon": -122.065561,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_516c4b5170316361516932524159686d5a63655245624a4a496843",
          "venue_name": "Benihana - Concord",
          "lat": 37.9699862,
          "lon": -122.0579469,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6b566f635f32787a757076524159686870754447582d454a496843",
          "venue_name": "The Cheesecake Factory",
          "lat": 37.8957855,
          "lon": -122.0619747,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_635257637977396f6c494b524159686e5a4352533133734a496843",
          "venue_name": "The Old Spaghetti Factory",
          "lat": 37.9770177,
          "lon": -122.0347282,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_38787571646b687533626d524159686e313478307464764a496843",
          "venue_name": "Dave & Buster's Concord - USA",
          "lat": 37.9730534,
          "lon": -122.0606711,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_67476b6869464c54374a4f524159686d39636f6a3033794a496843",
          "venue_name": "Denny's Restaurant",
          "lat": 37.9690545,
          "lon": -122.0523655,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_3064364a6633424954316c524159686d64634a66517a524a496843",
          "venue_name": "Super Duper Burgers",
          "lat": 37.972683,
          "lon": -122.0573339,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_455163596a75354c4a6f33524159686d354e32676666794a496843",
          "venue_name": "Chili's Grill & Bar",
          "lat": 37.978937,
          "lon": -122.041305,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6b61417847715a574a2d63524159686e566978726a58384a496843",
          "venue_name": "La Piñata 6",
          "lat": 37.9778258,
          "lon": -122.0317101,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_416846327144765735464752415968684a7354335732324a496843",
          "venue_name": "True Food Kitchen",
          "lat": 37.8952482,
          "lon": -122.0569626,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    },
    "34.42,-119.70,15000": {
      "at_unix": 1779891000,
      "venues": [
        {
          "venue_id": "ven_673046736c51537276756352415933707372584e7652534a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 34.4429318,
          "lon": -119.790712,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_736533596238517a435f4c5241593656494c76716a36384a496843",
          "venue_name": "Boathouse at Hendry's Beach",
          "lat": 34.4032167,
          "lon": -119.7438563,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_41556c692d334f4d375f3652415936545576754e4137474a496843",
          "venue_name": "Brophy Bros. Santa Barbara",
          "lat": 34.40397,
          "lon": -119.6933211,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_456663626953544b6e386f52415936544976703574582d4a496843",
          "venue_name": "Santa Barbara Shellfish Company",
          "lat": 34.408733,
          "lon": -119.684992,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_514264364f6e525f33304e524159365477594b436c37304a496843",
          "venue_name": "Santa Barbara FisHouse",
          "lat": 34.4135406,
          "lon": -119.688083,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_594e4f5a454f62307164595241593654493437686f2d364a496843",
          "venue_name": "Los Agaves Restaurant",
          "lat": 34.4274979,
          "lon": -119.6866222,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_38336c5a494c413476416952415936546349386a5339674a496843",
          "venue_name": "Sandbar Cocina y Tequila",
          "lat": 34.4172222,
          "lon": -119.6958333,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_41303253315f4b794773705241593654496672564632514a496843",
          "venue_name": "Moby Dick Restaurant",
          "lat": 34.4091814,
          "lon": -119.6852955,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_3841345a36447479462d475241593656595371536358514a496843",
          "venue_name": "Lure Fish House",
          "lat": 34.4387112,
          "lon": -119.7480354,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6745457a3459713037747552415936556b2d786e6a2d434a496843",
          "venue_name": "Los Agaves",
          "lat": 34.4374619,
          "lon": -119.7273729,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    },
    "37.49,-121.93,15000": {
      "at_unix": 1779891001,
      "venues": [
        {
          "venue_id": "ven_416966394253422d6535765241346a77694b58512d56354a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.420941,
          "lon": -122.0933529,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_45777057304232674a43625241346a4f7673696b6f39534a496843",
          "venue_name": "Dave & Buster's Milpitas - San Jose",
          "lat": 37.417728,
          "lon": -121.8979516,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_554367754e4533444a72315241346a3232567241412d534a496843",
          "venue_name": "Dishdash Middle Eastern Cuisine",
          "lat": 37.376177,
          "lon": -122.030136,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_3842657672563075376d355241346a336953506a39646e4a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.3804068,
          "lon": -122.0740341,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_41444145357567514538375241346a33537a64445454344a496843",
          "venue_name": "Crepevine Restaurants",
          "lat": 37.3925221,
          "lon": -122.0800081,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_7744566d48707831795f6352416f6d557a446e414837504a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.5992728,
          "lon": -122.0655899,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_73542d46703567556141635241346a3357466a506e38514a496843",
          "venue_name": "Zareen's",
          "lat": 37.4162147,
          "lon": -122.0795314,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_384247334a3852504a52395241346a3353544a5f4a52734a496843",
          "venue_name": "Oren's Hummus",
          "lat": 37.3947415,
          "lon": -122.0786015,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6f7139453234736d644b4f5241346a4f583938415762694a496843",
          "venue_name": "El Torito",
          "lat": 37.4331187,
          "lon": -121.897477,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_51726879395468445867615241346a32796c53676452444a496843",
          "venue_name": "Inchin's Bamboo Garden",
          "lat": 37.3762305,
          "lon": -122.0310557,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    },
    "37.44,-122.16,15000": {
      "at_unix": 1779891001,
      "venues": [
        {
          "venue_id": "ven_416966394253422d6535765241346a77694b58512d56354a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.420941,
          "lon": -122.0933529,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_554367754e4533444a72315241346a3232567241412d534a496843",
          "venue_name": "Dishdash Middle Eastern Cuisine",
          "lat": 37.376177,
          "lon": -122.030136,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_45557143447056425855635241346a36572d6c503978634a496843",
          "venue_name": "Zareen's Palo Alto",
          "lat": 37.4267457,
          "lon": -122.1440822,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_3842657672563075376d355241346a336953506a39646e4a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.3804068,
          "lon": -122.0740341,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_38555952786a41356c6b685241346a31693536496a5f794a496843",
          "venue_name": "Benihana - Cupertino",
          "lat": 37.3275364,
          "lon": -122.0137843,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_41444145357567514538375241346a33537a64445454344a496843",
          "venue_name": "Crepevine Restaurants",
          "lat": 37.3925221,
          "lon": -122.0800081,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_73542d46703567556141635241346a3357466a506e38514a496843",
          "venue_name": "Zareen's",
          "lat": 37.4162147,
          "lon": -122.0795314,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_384247334a3852504a52395241346a3353544a5f4a52734a496843",
          "venue_name": "Oren's Hummus",
          "lat": 37.3947415,
          "lon": -122.0786015,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_49764f4b507a6b463049465241346a7775356b543954314a496843",
          "venue_name": "Chef Chu's",
          "lat": 37.4005779,
          "lon": -122.1136827,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_51726879395468445867615241346a32796c53676452444a496843",
          "venue_name": "Inchin's Bamboo Garden",
          "lat": 37.3762305,
          "lon": -122.0310557,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    },
    "34.13,-118.26,15000": {
      "at_unix": 1779891004,
      "venues": [
        {
          "venue_id": "ven_416263334d7a596335384e52416f775f61796b527161444a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 34.0982287,
          "lon": -118.3416747,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_736d72793455315955506152416f77484c6e72644158744a496843",
          "venue_name": "Original Tommy's",
          "lat": 34.0695383,
          "lon": -118.2764832,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_59336756572d554f637a5952416f77362d4446316837444a496843",
          "venue_name": "Tito’s Tacos",
          "lat": 34.0081173,
          "lon": -118.4144858,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_51764e37796932664a355852416f77506a314b763251464a496843",
          "venue_name": "El Mercadito Mariachi Restaurant",
          "lat": 34.0370252,
          "lon": -118.1939043,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_776864446770593437516752416f7736714346566862774a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 34.0264826,
          "lon": -118.394275,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_306f4b4e765f684b2d694352416f775f32384b6d67544b4a496843",
          "venue_name": "Bob's Big Boy",
          "lat": 34.1524797,
          "lon": -118.3461035,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_383763666132475471766e524134776f783258782d33414a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 34.1350792,
          "lon": -118.3605992,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_4136706578776344334e7352416f7755365372686d637a4a496843",
          "venue_name": "Salsa & Beer",
          "lat": 34.2014561,
          "lon": -118.3869242,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_5965424d766874354a4b7252416f77342d486c5750534b4a496843",
          "venue_name": "Guelaguetza Restaurant",
          "lat": 34.0524074,
          "lon": -118.3006681,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_514c6b503147786179496452416f77415f763559672d544a496843",
          "venue_name": "Raffi's Place",
          "lat": 34.1465559,
          "lon": -118.2534519,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    },
    "33.84,-84.37,15000": {
      "at_unix": 1779891001,
      "venues": [
        {
          "venue_id": "ven_45536b4276725a4c6f3462526759394673376b527135524a496843",
          "venue_name": "The Hive Buckhead",
          "lat": 33.8054858,
          "lon": -84.3937103,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_51612d6f4c73474570473352675939454d304a61642d714a496843",
          "venue_name": "Bulla Gastrobar Atlanta",
          "lat": 33.7835243,
          "lon": -84.3848268,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_777737306e747530515453526759394645457a584a57394a496843",
          "venue_name": "Delbar - Inman Park",
          "lat": 33.7615997,
          "lon": -84.3602065,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_516f6f6243745a2d76435652675939525964665a3635534a496843",
          "venue_name": "Ray's on the River",
          "lat": 33.900492,
          "lon": -84.440802,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_4d6e44673153514e704a545267593946304a77386e372d4a496843",
          "venue_name": "Pelicana Chicken Atlanta",
          "lat": 33.7859285,
          "lon": -84.4008723,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_4d36483247763367772d61526759396e435946592d32474a496843",
          "venue_name": "E-Gyu All You Can Eat Revolving Sushi & Korean BBQ - Doraville",
          "lat": 33.9137463,
          "lon": -84.2613826,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_30376b69525466454456705267593945304835333334594a496843",
          "venue_name": "Atlanta Breakfast Club",
          "lat": 33.7648612,
          "lon": -84.3954381,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_4d3257426a503475666773526759394630596b6b4f58524a496843",
          "venue_name": "Atlanta Fish Market",
          "lat": 33.8366606,
          "lon": -84.3787991,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_7348694f44324c565f7934526759394a383149395f36684a496843",
          "venue_name": "Fogo de Chão Brazilian Steakhouse",
          "lat": 33.9310446,
          "lon": -84.3370298,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6754586e456f544756514252675939476736797a5738474a496843",
          "venue_name": "The Vortex Bar & Grill",
          "lat": 33.7662472,
          "lon": -84.3492306,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    },
    "32.91,-117.11,15000": {
      "at_unix": 1779891001,
      "venues": [
        {
          "venue_id": "ven_455048316635436d726767524134325f5470496c4e54414a496843",
          "venue_name": "Steamy Piggy",
          "lat": 32.8256311,
          "lon": -117.1545801,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_676a5979784a6a77743643524134325f4838794f7765774a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 32.8196199,
          "lon": -117.1486875,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_776574717335444438715852415932593954572d4e33324a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 32.808182,
          "lon": -116.9629357,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_636f6e53304d3458706369524159325a6c38505f6d63484a496843",
          "venue_name": "Golden Corral Buffet & Grill",
          "lat": 32.7952558,
          "lon": -116.9662569,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_514279744b4a77367336785241493344735066724357724a496843",
          "venue_name": "Puesto La Jolla",
          "lat": 32.8469862,
          "lon": -117.2738346,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_30706b6a55525f7266494952413432357a42476c5855384a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 32.9176848,
          "lon": -117.1219784,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_4d6e6d3677456742495f38524134325f54705a5349596f4a496843",
          "venue_name": "Kura Revolving Sushi Bar",
          "lat": 32.8240891,
          "lon": -117.1547553,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_4d4a454849305a5544574d52414933506f6b71557962684a496843",
          "venue_name": "Jake's Del Mar",
          "lat": 32.961908,
          "lon": -117.268087,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_414a4a6330784a6147484f52415932584a5277434338434a496843",
          "venue_name": "BJ's Restaurant & Brewhouse",
          "lat": 32.780023,
          "lon": -117.0101801,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_51694f6671624d5f785a36525949524b6e67666f73514d4a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 32.8389813,
          "lon": -116.9939247,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    },
    "37.67,-122.47,15000": {
      "at_unix": 1779891002,
      "venues": [
        {
          "venue_id": "ven_38377338374e4e487550515241596841477645634858484a496843",
          "venue_name": "Tony's Pizza Napoletana",
          "lat": 37.8003815,
          "lon": -122.4090457,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_51785f397358495a6d366d5241346a384a6e69796a56334a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.6882276,
          "lon": -122.4720041,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_346a386131624b6a5155325241596841716561633736674a496843",
          "venue_name": "House of Prime Rib",
          "lat": 37.7934329,
          "lon": -122.422732,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_45435f707a5735425265645241346a2d5a5531425954724a496843",
          "venue_name": "La Taqueria",
          "lat": 37.7508961,
          "lon": -122.4180867,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_774235764e7178555f4f6f52415968416934597467564c4a496843",
          "venue_name": "Super Duper Burgers",
          "lat": 37.7868541,
          "lon": -122.4039683,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_4d385265655438366c63495241346a2d645554495257354a496843",
          "venue_name": "Taquería El Farolito",
          "lat": 37.7526463,
          "lon": -122.4183182,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_55787463654d444958452d5241346a37744f56716831724a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.6658291,
          "lon": -122.4693245,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_514b5731426f626f77564a5241596841477656795966514a496843",
          "venue_name": "Sotto Mare",
          "lat": 37.7997503,
          "lon": -122.4083449,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6f4b696145557a4d41304e524159684165324e4b4452554a496843",
          "venue_name": "La Mar Cocina Peruana San Francisco",
          "lat": 37.797239,
          "lon": -122.3953922,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_59656e4f5576347553386252415968412d5956754d39654a496843",
          "venue_name": "John's Grill",
          "lat": 37.7854407,
          "lon": -122.4071974,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    },
    "37.26,-121.87,15000": {
      "at_unix": 1779891001,
      "venues": [
        {
          "venue_id": "ven_554367754e4533444a72315241346a3232567241412d534a496843",
          "venue_name": "Dishdash Middle Eastern Cuisine",
          "lat": 37.376177,
          "lon": -122.030136,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_38555952786a41356c6b685241346a31693536496a5f794a496843",
          "venue_name": "Benihana - Cupertino",
          "lat": 37.3275364,
          "lon": -122.0137843,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_49475f2d626f30453165705241346a4c37784e6c4957574a496843",
          "venue_name": "Din Tai Fung 鼎泰豐",
          "lat": 37.3262207,
          "lon": -121.9441336,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6b466a536a634e337674675263344b787557566d7953704a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.3300865,
          "lon": -121.8113184,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_386632706354494b6731575241346a4c6a47576859544f4a496843",
          "venue_name": "LUNA Mexican Kitchen - San Jose",
          "lat": 37.3339808,
          "lon": -121.9152907,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6359467478574b7135745352416f6a7a637a4e324b64664a496843",
          "venue_name": "Denny's Restaurant",
          "lat": 37.3168244,
          "lon": -121.8738022,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_51726879395468445867615241346a32796c53676452444a496843",
          "venue_name": "Inchin's Bamboo Garden",
          "lat": 37.3762305,
          "lon": -122.0310557,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_493656694a6c7535456d645241346a4c5f4e6f4e52345a4a496843",
          "venue_name": "Dish N Dash",
          "lat": 37.3843966,
          "lon": -121.9272943,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6b655a6d6d58684f33694b5241346a4c6a6f727659394b4a496843",
          "venue_name": "La Victoria Taquería",
          "lat": 37.3634927,
          "lon": -121.9070037,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_34564e583142586858663052416f6a7a7378754d5a576d4a496843",
          "venue_name": "The Boiling Crab",
          "lat": 37.3026313,
          "lon": -121.864222,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    },
    "37.39,-121.95,15000": {
      "at_unix": 1779891000,
      "venues": [
        {
          "venue_id": "ven_416966394253422d6535765241346a77694b58512d56354a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.420941,
          "lon": -122.0933529,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_45777057304232674a43625241346a4f7673696b6f39534a496843",
          "venue_name": "Dave & Buster's Milpitas - San Jose",
          "lat": 37.417728,
          "lon": -121.8979516,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_554367754e4533444a72315241346a3232567241412d534a496843",
          "venue_name": "Dishdash Middle Eastern Cuisine",
          "lat": 37.376177,
          "lon": -122.030136,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_3842657672563075376d355241346a336953506a39646e4a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.3804068,
          "lon": -122.0740341,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_38555952786a41356c6b685241346a31693536496a5f794a496843",
          "venue_name": "Benihana - Cupertino",
          "lat": 37.3275364,
          "lon": -122.0137843,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_49475f2d626f30453165705241346a4c37784e6c4957574a496843",
          "venue_name": "Din Tai Fung 鼎泰豐",
          "lat": 37.3262207,
          "lon": -121.9441336,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_41444145357567514538375241346a33537a64445454344a496843",
          "venue_name": "Crepevine Restaurants",
          "lat": 37.3925221,
          "lon": -122.0800081,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6b466a536a634e337674675263344b787557566d7953704a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 37.3300865,
          "lon": -121.8113184,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_386632706354494b6731575241346a4c6a47576859544f4a496843",
          "venue_name": "LUNA Mexican Kitchen - San Jose",
          "lat": 37.3339808,
          "lon": -121.9152907,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_73542d46703567556141635241346a3357466a506e38514a496843",
          "venue_name": "Zareen's",
          "lat": 37.4162147,
          "lon": -122.0795314,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    },
    "36.98,-122.03,15000": {
      "at_unix": 1779891000,
      "venues": [
        {
          "venue_id": "ven_553851715644645058386c52416f6a4156696e334c52454a496843",
          "venue_name": "Taqueria Los Pericos",
          "lat": 36.977913,
          "lon": -122.0258226,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6b5539725349736b494b5452416f6a41647978387530544a496843",
          "venue_name": "Pizza My Heart",
          "lat": 36.972285,
          "lon": -122.025417,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_49766a7a4c4e6c706c634352416f6a71704a5f424b774f4a496843",
          "venue_name": "STAGNARO BROS. SEAFOOD, INC.",
          "lat": 36.9582529,
          "lon": -122.01781,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_34303443754c377979327852416f6a71704a616d59374e4a496843",
          "venue_name": "Ideal Bar & Grill",
          "lat": 36.9627127,
          "lon": -122.0232801,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_555f774d524d645845716452416f6a715a3533744678494a496843",
          "venue_name": "Riva Fish House",
          "lat": 36.9598194,
          "lon": -122.0196306,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_303271353534536c77437152416f6a415a5362762d2d774a496843",
          "venue_name": "Woodstock's Pizza Santa Cruz",
          "lat": 36.9747836,
          "lon": -122.0250499,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_416d637a743375637a445552416f6a56734162506c364a4a496843",
          "venue_name": "Zelda's on the beach",
          "lat": 36.9718922,
          "lon": -121.9518947,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6b36506c56447768576d2d52416f6a56595764414754524a496843",
          "venue_name": "Pizza My Heart",
          "lat": 36.9807537,
          "lon": -121.9645503,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_456e52505f62553761325152416f6a715271782d3955504a496843",
          "venue_name": "Betty Burgers",
          "lat": 36.9670602,
          "lon": -122.0080642,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_7349387759766a362d387852416f6a7164354d754b38484a496843",
          "venue_name": "Makai Island Kitchen & Groggery",
          "lat": 36.9589407,
          "lon": -122.0184181,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    },
    "38.46,-121.42,15000": {
      "at_unix": 1779891001,
      "venues": [
        {
          "venue_id": "ven_774d69526959795236527852416f6d4f483263433758794a496843",
          "venue_name": "In-N-Out Burger",
          "lat": 38.4627315,
          "lon": -121.4918759,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_4966614d656d347773412d52416f6d523778646d57344d4a496843",
          "venue_name": "Tower Cafe",
          "lat": 38.5616745,
          "lon": -121.4935217,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6f59704d6751794e71694152416f6d5262536e4e707a374a496843",
          "venue_name": "Fox & Goose Public House",
          "lat": 38.5715893,
          "lon": -121.4969745,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_4d356c47495a2d614f337052416f6d5176746a466d5f304a496843",
          "venue_name": "Zócalo Midtown",
          "lat": 38.5742377,
          "lon": -121.4836787,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_344865434e594230716a5752416f6d52374242706334634a496843",
          "venue_name": "Willie's Burgers",
          "lat": 38.5820723,
          "lon": -121.5055909,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_346c35585a4c673471413052416f6d47376b755053396a4a496843",
          "venue_name": "Chevys",
          "lat": 38.4239971,
          "lon": -121.4163422,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_346d68365052674b44493852416f6d514c734c5750385f4a496843",
          "venue_name": "Tres Hermanas",
          "lat": 38.573873,
          "lon": -121.474514,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_6b71644c7a7171682d417952416f6d616a6a6e376356544a496843",
          "venue_name": "Denny's Restaurant",
          "lat": 38.5897248,
          "lon": -121.414917,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_556255544b4d6b346a4c5a52416f6d5176646e743536694a496843",
          "venue_name": "319 Broderick",
          "lat": 38.5882245,
          "lon": -121.5140309,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_594b6f627072494a38455552416f6d517a75386a6e594c4a496843",
          "venue_name": "Suzie Burger",
          "lat": 38.5666241,
          "lon": -121.4712796,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    },
    "37.54,-122.29,15000": {
      "at_unix": 1779891001,
      "venues": [
        {
          "venue_id": "ven_45557143447056425855635241346a36572d6c503978634a496843",
          "venue_name": "Zareen's Palo Alto",
          "lat": 37.4267457,
          "lon": -122.1440822,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_593741533143317745784b5241346a3231794e4f7733724a496843",
          "venue_name": "Benihana",
          "lat": 37.5982151,
          "lon": -122.3642571,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_452d574d3570546231626a5241346a37575472494b39744a496843",
          "venue_name": "True Food Kitchen",
          "lat": 37.443921,
          "lon": -122.170323,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_38694f54435658797251315241346a65576d41312d51484a496843",
          "venue_name": "Espetus Churrascaria Brazilian Steakhouse - San Mateo",
          "lat": 37.5622986,
          "lon": -122.3192993,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_41714f637134767a6c69385241346a664f6745625279674a496843",
          "venue_name": "The Cheesecake Factory",
          "lat": 37.5364894,
          "lon": -122.2990469,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_67725a433249364a5862745241346a376d6a347577576d4a496843",
          "venue_name": "Rangoon Ruby Burmese Cuisine",
          "lat": 37.4450763,
          "lon": -122.1631074,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_63594a356c306d357376545241346a6447762d566d30554a496843",
          "venue_name": "Limón",
          "lat": 37.5795444,
          "lon": -122.3454031,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_5962766a4b3345314450585241346a3552636f4643784b4a496843",
          "venue_name": "Chili's Grill & Bar",
          "lat": 37.630417,
          "lon": -122.418185,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_304865784e554a724976575241346a35647347756f58644a496843",
          "venue_name": "BJ's Restaurant & Brewhouse",
          "lat": 37.6354885,
          "lon": -122.418822,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        },
        {
          "venue_id": "ven_554a6b7a306166703473755241346a37537a4f595639444a496843",
          "venue_name": "The Melt",
          "lat": 37.4430947,
          "lon": -122.1725973,
          "live_busyness": 0.0,
          "forecast_busyness": 0.0
        }
      ]
    }
  }
}
"##;
