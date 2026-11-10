//! Provider identities from FOSS src/provider/driver-profiles.ts. No settings or credentials.
use super::OperatorProfile;
pub const PROFILES: &[OperatorProfile] = &[
    OperatorProfile {
        id: "demo",
        kind: "demo",
        legacy_prefix: "demo",
        title: "Demo — moving test pattern",
    },
    OperatorProfile {
        id: "offline",
        kind: "offline",
        legacy_prefix: "offline",
        title: "Local videos",
    },
    OperatorProfile {
        id: "xtream",
        kind: "xtream",
        legacy_prefix: "xtream",
        title: "Xtream Codes",
    },
    OperatorProfile {
        id: "stalker",
        kind: "stalker",
        legacy_prefix: "stalker",
        title: "Stalker portal",
    },
    OperatorProfile {
        id: "m3u",
        kind: "m3u",
        legacy_prefix: "m3u",
        title: "M3U playlist",
    },
    OperatorProfile {
        id: "plex",
        kind: "plex",
        legacy_prefix: "plex",
        title: "Plex",
    },
    OperatorProfile {
        id: "vportal",
        kind: "vportal",
        legacy_prefix: "vportal",
        title: "VPortal",
    },
    OperatorProfile {
        id: "torrent",
        kind: "torrent",
        legacy_prefix: "torrent",
        title: "Torrent",
    },
    OperatorProfile {
        id: "antifriz",
        kind: "playlist",
        legacy_prefix: "az",
        title: "Antifriz",
    },
    OperatorProfile {
        id: "kb-team",
        kind: "playlist",
        legacy_prefix: "kbc",
        title: "KBC (Kinoboom)",
    },
    OperatorProfile {
        id: "edem",
        kind: "edem",
        legacy_prefix: "ed",
        title: "Edem.tv / iLook.tv",
    },
    OperatorProfile {
        id: "itv",
        kind: "catalog",
        legacy_prefix: "itv",
        title: "1Cent.tv",
    },
    OperatorProfile {
        id: "ottclub",
        kind: "catalog",
        legacy_prefix: "",
        title: "OTTCLUB",
    },
    OperatorProfile {
        id: "shura",
        kind: "catalog",
        legacy_prefix: "sh",
        title: "SHURATV",
    },
    OperatorProfile {
        id: "1ott",
        kind: "named-playlist",
        legacy_prefix: "1ott",
        title: "1OTT.NET",
    },
    OperatorProfile {
        id: "bestlist/stalker",
        kind: "xtream-fallback",
        legacy_prefix: "bestlist_stalker",
        title: "BEST LiST IPTV [Stalker]",
    },
    OperatorProfile {
        id: "only4",
        kind: "named-playlist",
        legacy_prefix: "o4",
        title: "Only4.tv",
    },
    OperatorProfile {
        id: "shara-tv",
        kind: "named-playlist",
        legacy_prefix: "shtv",
        title: "shara-tv",
    },
    OperatorProfile {
        id: "tvteam",
        kind: "named-playlist",
        legacy_prefix: "tvteam",
        title: "tv.team",
    },
    OperatorProfile {
        id: "all4you",
        kind: "operator",
        legacy_prefix: "all4you",
        title: "All4you.tv",
    },
    OperatorProfile {
        id: "bestlist",
        kind: "operator",
        legacy_prefix: "bestlist",
        title: "BEST LiST IPTV [HLS Playlist]",
    },
    OperatorProfile {
        id: "cbilling",
        kind: "operator",
        legacy_prefix: "cbilling",
        title: "Гомельсат (cbilling)",
    },
    OperatorProfile {
        id: "d/maxtv",
        kind: "operator",
        legacy_prefix: "d_maxtv",
        title: "MaxTV",
    },
    OperatorProfile {
        id: "diamondtv",
        kind: "operator",
        legacy_prefix: "diamondtv",
        title: "Diamond TV",
    },
    OperatorProfile {
        id: "dosug",
        kind: "operator",
        legacy_prefix: "dosug",
        title: "TV DOSUG",
    },
    OperatorProfile {
        id: "dragon",
        kind: "operator",
        legacy_prefix: "dragon",
        title: "Dragon Media PRO",
    },
    OperatorProfile {
        id: "drvao",
        kind: "operator",
        legacy_prefix: "drvao",
        title: "DRVAO",
    },
    OperatorProfile {
        id: "fabryka",
        kind: "operator",
        legacy_prefix: "fabryka",
        title: "Fabryka.TV",
    },
    OperatorProfile {
        id: "fox",
        kind: "operator",
        legacy_prefix: "fox",
        title: "Fox-TV",
    },
    OperatorProfile {
        id: "fxml",
        kind: "operator",
        legacy_prefix: "fxml",
        title: "FXML",
    },
    OperatorProfile {
        id: "great",
        kind: "operator",
        legacy_prefix: "great",
        title: "GREAT IPTV",
    },
    OperatorProfile {
        id: "ipstream",
        kind: "operator",
        legacy_prefix: "ipstream",
        title: "IpStream.one",
    },
    OperatorProfile {
        id: "iptv-ott.ru",
        kind: "operator",
        legacy_prefix: "iptv-ott.ru",
        title: "VIP-IP.COM",
    },
    OperatorProfile {
        id: "korona",
        kind: "operator",
        legacy_prefix: "korona",
        title: "KORONA TV",
    },
    OperatorProfile {
        id: "moidom",
        kind: "operator",
        legacy_prefix: "moidom",
        title: "МойДом",
    },
    OperatorProfile {
        id: "newlook",
        kind: "operator",
        legacy_prefix: "newlook",
        title: "New Look",
    },
    OperatorProfile {
        id: "ottg",
        kind: "operator",
        legacy_prefix: "ottg",
        title: "GlanzTV",
    },
    OperatorProfile {
        id: "ottprime",
        kind: "operator",
        legacy_prefix: "ottprime",
        title: "OTT Prime ONLINE",
    },
    OperatorProfile {
        id: "polmedia",
        kind: "operator",
        legacy_prefix: "polmedia",
        title: "POLMEDIA",
    },
    OperatorProfile {
        id: "prost",
        kind: "operator",
        legacy_prefix: "prost",
        title: "PROST",
    },
    OperatorProfile {
        id: "raduga",
        kind: "operator",
        legacy_prefix: "raduga",
        title: "Радуга",
    },
    OperatorProfile {
        id: "rd",
        kind: "operator",
        legacy_prefix: "rd",
        title: "RD",
    },
    OperatorProfile {
        id: "russkoetv",
        kind: "operator",
        legacy_prefix: "russkoetv",
        title: "RUSSKOETV",
    },
    OperatorProfile {
        id: "shara.club",
        kind: "operator",
        legacy_prefix: "shara_club",
        title: "Shara.club (ClubTV.pro)",
    },
    OperatorProfile {
        id: "sharavoz",
        kind: "operator",
        legacy_prefix: "sharavoz",
        title: "Шаравоз",
    },
    OperatorProfile {
        id: "shocktv",
        kind: "operator",
        legacy_prefix: "shocktv",
        title: "ShockTv",
    },
    OperatorProfile {
        id: "tabox",
        kind: "operator",
        legacy_prefix: "tabox",
        title: "Tabox",
    },
    OperatorProfile {
        id: "top",
        kind: "operator",
        legacy_prefix: "top",
        title: "Top-Tv",
    },
    OperatorProfile {
        id: "topiptv",
        kind: "operator",
        legacy_prefix: "topiptv",
        title: "TOP-IPTV",
    },
    OperatorProfile {
        id: "tvclub",
        kind: "operator",
        legacy_prefix: "tvclub",
        title: "TVClub",
    },
    OperatorProfile {
        id: "ultifl1x",
        kind: "operator",
        legacy_prefix: "ultifl1x",
        title: "ULTIFL1X",
    },
    OperatorProfile {
        id: "vidok",
        kind: "operator",
        legacy_prefix: "vidok",
        title: "Vidok.TV",
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
pub enum OperatorId {
    #[serde(rename = "antifriz")]
    Antifriz,
    #[serde(rename = "kb-team")]
    KbTeam,
    #[serde(rename = "itv")]
    Itv,
    #[serde(rename = "ottclub")]
    Ottclub,
    #[serde(rename = "shura")]
    Shura,
    #[serde(rename = "1ott")]
    OneOtt,
    #[serde(rename = "bestlist/stalker")]
    BestlistStalker,
    #[serde(rename = "only4")]
    Only4,
    #[serde(rename = "shara-tv")]
    SharaTv,
    #[serde(rename = "tvteam")]
    Tvteam,
    #[serde(rename = "all4you")]
    All4you,
    #[serde(rename = "bestlist")]
    Bestlist,
    #[serde(rename = "cbilling")]
    Cbilling,
    #[serde(rename = "d/maxtv")]
    DMaxtv,
    #[serde(rename = "diamondtv")]
    Diamondtv,
    #[serde(rename = "dosug")]
    Dosug,
    #[serde(rename = "dragon")]
    Dragon,
    #[serde(rename = "drvao")]
    Drvao,
    #[serde(rename = "fabryka")]
    Fabryka,
    #[serde(rename = "fox")]
    Fox,
    #[serde(rename = "fxml")]
    Fxml,
    #[serde(rename = "great")]
    Great,
    #[serde(rename = "ipstream")]
    Ipstream,
    #[serde(rename = "iptv-ott.ru")]
    IptvOttRu,
    #[serde(rename = "korona")]
    Korona,
    #[serde(rename = "moidom")]
    Moidom,
    #[serde(rename = "newlook")]
    Newlook,
    #[serde(rename = "ottg")]
    Ottg,
    #[serde(rename = "ottprime")]
    Ottprime,
    #[serde(rename = "polmedia")]
    Polmedia,
    #[serde(rename = "prost")]
    Prost,
    #[serde(rename = "raduga")]
    Raduga,
    #[serde(rename = "rd")]
    Rd,
    #[serde(rename = "russkoetv")]
    Russkoetv,
    #[serde(rename = "shara.club")]
    SharaClub,
    #[serde(rename = "sharavoz")]
    Sharavoz,
    #[serde(rename = "shocktv")]
    Shocktv,
    #[serde(rename = "tabox")]
    Tabox,
    #[serde(rename = "top")]
    Top,
    #[serde(rename = "topiptv")]
    Topiptv,
    #[serde(rename = "tvclub")]
    Tvclub,
    #[serde(rename = "ultifl1x")]
    Ultifl1x,
    #[serde(rename = "vidok")]
    Vidok,
}
impl OperatorId {
    pub const ALL: &'static [Self] = &[
        Self::Antifriz,
        Self::KbTeam,
        Self::Itv,
        Self::Ottclub,
        Self::Shura,
        Self::OneOtt,
        Self::BestlistStalker,
        Self::Only4,
        Self::SharaTv,
        Self::Tvteam,
        Self::All4you,
        Self::Bestlist,
        Self::Cbilling,
        Self::DMaxtv,
        Self::Diamondtv,
        Self::Dosug,
        Self::Dragon,
        Self::Drvao,
        Self::Fabryka,
        Self::Fox,
        Self::Fxml,
        Self::Great,
        Self::Ipstream,
        Self::IptvOttRu,
        Self::Korona,
        Self::Moidom,
        Self::Newlook,
        Self::Ottg,
        Self::Ottprime,
        Self::Polmedia,
        Self::Prost,
        Self::Raduga,
        Self::Rd,
        Self::Russkoetv,
        Self::SharaClub,
        Self::Sharavoz,
        Self::Shocktv,
        Self::Tabox,
        Self::Top,
        Self::Topiptv,
        Self::Tvclub,
        Self::Ultifl1x,
        Self::Vidok,
    ];
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|p| p.canonical_id() == id)
    }
    pub fn profile(self) -> &'static OperatorProfile {
        &PROFILES[match self {
            Self::Antifriz => 8,
            Self::KbTeam => 9,
            Self::Itv => 11,
            Self::Ottclub => 12,
            Self::Shura => 13,
            Self::OneOtt => 14,
            Self::BestlistStalker => 15,
            Self::Only4 => 16,
            Self::SharaTv => 17,
            Self::Tvteam => 18,
            Self::All4you => 19,
            Self::Bestlist => 20,
            Self::Cbilling => 21,
            Self::DMaxtv => 22,
            Self::Diamondtv => 23,
            Self::Dosug => 24,
            Self::Dragon => 25,
            Self::Drvao => 26,
            Self::Fabryka => 27,
            Self::Fox => 28,
            Self::Fxml => 29,
            Self::Great => 30,
            Self::Ipstream => 31,
            Self::IptvOttRu => 32,
            Self::Korona => 33,
            Self::Moidom => 34,
            Self::Newlook => 35,
            Self::Ottg => 36,
            Self::Ottprime => 37,
            Self::Polmedia => 38,
            Self::Prost => 39,
            Self::Raduga => 40,
            Self::Rd => 41,
            Self::Russkoetv => 42,
            Self::SharaClub => 43,
            Self::Sharavoz => 44,
            Self::Shocktv => 45,
            Self::Tabox => 46,
            Self::Top => 47,
            Self::Topiptv => 48,
            Self::Tvclub => 49,
            Self::Ultifl1x => 50,
            Self::Vidok => 51,
        }]
    }
    pub fn canonical_id(self) -> &'static str {
        self.profile().id
    }
    pub fn name(self) -> &'static str {
        self.profile().title
    }
    pub fn driver_kind(self) -> &'static str {
        self.profile().kind
    }
    pub fn named_profile(self) -> Option<super::NamedProfile> {
        match self {
            Self::Antifriz => Some(super::NamedProfile::Antifriz),
            Self::KbTeam => Some(super::NamedProfile::KbTeam),
            Self::Shura => Some(super::NamedProfile::Shura),
            Self::OneOtt => Some(super::NamedProfile::OneOtt),
            Self::Only4 => Some(super::NamedProfile::Only4),
            Self::SharaTv => Some(super::NamedProfile::SharaTv),
            Self::Tvteam => Some(super::NamedProfile::Tvteam),
            _ => None,
        }
    }
}
