//! Closed operator identities mirrored from the pinned FOSS provider registry.
//! Application contract tests compare these values with the canonical shared core.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    pub const ALL: &[Self] = &[
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
    pub const fn canonical_id(self) -> &'static str {
        match self {
            Self::Antifriz => "antifriz",
            Self::KbTeam => "kb-team",
            Self::Itv => "itv",
            Self::Ottclub => "ottclub",
            Self::Shura => "shura",
            Self::OneOtt => "1ott",
            Self::BestlistStalker => "bestlist/stalker",
            Self::Only4 => "only4",
            Self::SharaTv => "shara-tv",
            Self::Tvteam => "tvteam",
            Self::All4you => "all4you",
            Self::Bestlist => "bestlist",
            Self::Cbilling => "cbilling",
            Self::DMaxtv => "d/maxtv",
            Self::Diamondtv => "diamondtv",
            Self::Dosug => "dosug",
            Self::Dragon => "dragon",
            Self::Drvao => "drvao",
            Self::Fabryka => "fabryka",
            Self::Fox => "fox",
            Self::Fxml => "fxml",
            Self::Great => "great",
            Self::Ipstream => "ipstream",
            Self::IptvOttRu => "iptv-ott.ru",
            Self::Korona => "korona",
            Self::Moidom => "moidom",
            Self::Newlook => "newlook",
            Self::Ottg => "ottg",
            Self::Ottprime => "ottprime",
            Self::Polmedia => "polmedia",
            Self::Prost => "prost",
            Self::Raduga => "raduga",
            Self::Rd => "rd",
            Self::Russkoetv => "russkoetv",
            Self::SharaClub => "shara.club",
            Self::Sharavoz => "sharavoz",
            Self::Shocktv => "shocktv",
            Self::Tabox => "tabox",
            Self::Top => "top",
            Self::Topiptv => "topiptv",
            Self::Tvclub => "tvclub",
            Self::Ultifl1x => "ultifl1x",
            Self::Vidok => "vidok",
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::Antifriz => "Antifriz",
            Self::KbTeam => "KBC (Kinoboom)",
            Self::Itv => "1Cent.tv",
            Self::Ottclub => "OTTCLUB",
            Self::Shura => "SHURATV",
            Self::OneOtt => "1OTT.NET",
            Self::BestlistStalker => "BEST LiST IPTV [Stalker]",
            Self::Only4 => "Only4.tv",
            Self::SharaTv => "shara-tv",
            Self::Tvteam => "tv.team",
            Self::All4you => "All4you.tv",
            Self::Bestlist => "BEST LiST IPTV [HLS Playlist]",
            Self::Cbilling => "Гомельсат (cbilling)",
            Self::DMaxtv => "MaxTV",
            Self::Diamondtv => "Diamond TV",
            Self::Dosug => "TV DOSUG",
            Self::Dragon => "Dragon Media PRO",
            Self::Drvao => "DRVAO",
            Self::Fabryka => "Fabryka.TV",
            Self::Fox => "Fox-TV",
            Self::Fxml => "FXML",
            Self::Great => "GREAT IPTV",
            Self::Ipstream => "IpStream.one",
            Self::IptvOttRu => "VIP-IP.COM",
            Self::Korona => "KORONA TV",
            Self::Moidom => "МойДом",
            Self::Newlook => "New Look",
            Self::Ottg => "GlanzTV",
            Self::Ottprime => "OTT Prime ONLINE",
            Self::Polmedia => "POLMEDIA",
            Self::Prost => "PROST",
            Self::Raduga => "Радуга",
            Self::Rd => "RD",
            Self::Russkoetv => "RUSSKOETV",
            Self::SharaClub => "Shara.club (ClubTV.pro)",
            Self::Sharavoz => "Шаравоз",
            Self::Shocktv => "ShockTv",
            Self::Tabox => "Tabox",
            Self::Top => "Top-Tv",
            Self::Topiptv => "TOP-IPTV",
            Self::Tvclub => "TVClub",
            Self::Ultifl1x => "ULTIFL1X",
            Self::Vidok => "Vidok.TV",
        }
    }
    pub const fn driver_kind(self) -> &'static str {
        match self {
            Self::Antifriz => "playlist",
            Self::KbTeam => "playlist",
            Self::Itv => "catalog",
            Self::Ottclub => "catalog",
            Self::Shura => "catalog",
            Self::OneOtt => "named-playlist",
            Self::BestlistStalker => "xtream-fallback",
            Self::Only4 => "named-playlist",
            Self::SharaTv => "named-playlist",
            Self::Tvteam => "named-playlist",
            Self::All4you => "operator",
            Self::Bestlist => "operator",
            Self::Cbilling => "operator",
            Self::DMaxtv => "operator",
            Self::Diamondtv => "operator",
            Self::Dosug => "operator",
            Self::Dragon => "operator",
            Self::Drvao => "operator",
            Self::Fabryka => "operator",
            Self::Fox => "operator",
            Self::Fxml => "operator",
            Self::Great => "operator",
            Self::Ipstream => "operator",
            Self::IptvOttRu => "operator",
            Self::Korona => "operator",
            Self::Moidom => "operator",
            Self::Newlook => "operator",
            Self::Ottg => "operator",
            Self::Ottprime => "operator",
            Self::Polmedia => "operator",
            Self::Prost => "operator",
            Self::Raduga => "operator",
            Self::Rd => "operator",
            Self::Russkoetv => "operator",
            Self::SharaClub => "operator",
            Self::Sharavoz => "operator",
            Self::Shocktv => "operator",
            Self::Tabox => "operator",
            Self::Top => "operator",
            Self::Topiptv => "operator",
            Self::Tvclub => "operator",
            Self::Ultifl1x => "operator",
            Self::Vidok => "operator",
        }
    }
    pub fn from_id(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|id| id.canonical_id() == value)
    }
}
