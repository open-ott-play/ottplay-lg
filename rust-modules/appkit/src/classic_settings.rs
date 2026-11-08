//! Shared, checked Classic list and numeric-seek preferences. No storage or IO.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageSize(u8);
impl PageSize {
    pub fn from_rows(rows: usize) -> Option<Self> {
        (10..=30).contains(&rows).then_some(Self(rows as u8))
    }
    pub fn rows(self) -> usize { usize::from(self.0) }
}
impl Default for PageSize { fn default() -> Self { Self(25) } }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListPosition { Right, Left }
impl ListPosition {
    pub fn from_index(index: u8) -> Option<Self> {
        match index { 0 => Some(Self::Right), 1 => Some(Self::Left), _ => None }
    }
    pub fn index(self) -> u8 { match self { Self::Right => 0, Self::Left => 1 } }
}
impl Default for ListPosition { fn default() -> Self { Self::Right } }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrowFunction { Paging, Volume, Dune, LegacyNeutrino }
impl ArrowFunction {
    pub fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Paging), 1 => Some(Self::Volume),
            2 => Some(Self::Dune), 3 => Some(Self::LegacyNeutrino), _ => None,
        }
    }
    /// Legacy index 3 retains its stored identity; list behavior is Paging.
    pub fn index(self) -> u8 {
        match self { Self::Paging => 0, Self::Volume => 1, Self::Dune => 2, Self::LegacyNeutrino => 3 }
    }
}
impl Default for ArrowFunction { fn default() -> Self { Self::Paging } }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seek13Duration(u32);
impl Seek13Duration {
    pub const CHOICES: [u32; 15] = [5, 10, 15, 20, 30, 60, 120, 180, 240, 300, 600, 900, 1200, 1800, 3600];
    pub fn from_seconds(seconds: u32) -> Option<Self> {
        Self::CHOICES.contains(&seconds).then_some(Self(seconds))
    }
    pub fn seconds(self) -> u32 { self.0 }
}
impl Default for Seek13Duration { fn default() -> Self { Self(15) } }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preferences {
    page_size: PageSize,
    list_position: ListPosition,
    arrow_function: ArrowFunction,
    seek13_duration: Seek13Duration,
    show_number: bool,
    show_name: bool,
    show_program: bool,
    show_description: bool,
    show_progress: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self { page_size: Default::default(), list_position: Default::default(),
            arrow_function: Default::default(), seek13_duration: Default::default(),
            show_number: true, show_name: true, show_program: true, show_description: true, show_progress: true }
    }
}
impl Preferences {
    pub fn from_values(page_size: usize, list_position: u8, arrow_mode: u8, seek13_seconds: u32) -> Option<Self> {
        Some(Self {
            page_size: PageSize::from_rows(page_size)?,
            list_position: ListPosition::from_index(list_position)?,
            arrow_function: ArrowFunction::from_index(arrow_mode)?,
            seek13_duration: Seek13Duration::from_seconds(seek13_seconds)?,
            ..Self::default()
        })
    }
    /// Classic channel-list visibility, using the shared 0/1 settings contract.
    pub fn with_channel_display(mut self, number: u8, name: u8, program: u8, description: u8) -> Option<Self> {
        if [number, name, program, description].iter().any(|value| *value > 1) { return None; }
        self.show_number = number == 1;
        self.show_name = name == 1;
        self.show_program = program == 1;
        self.show_description = description == 1;
        Some(self)
    }
    pub fn show_number(&self) -> bool { self.show_number }
    pub fn show_name(&self) -> bool { self.show_name }
    pub fn show_program(&self) -> bool { self.show_program }
    pub fn show_description(&self) -> bool { self.show_description }
    /// Progress visibility is independent of current-programme title visibility.
    pub fn with_show_progress(mut self, progress: u8) -> Option<Self> {
        if progress > 1 { return None; }
        self.show_progress = progress == 1;
        Some(self)
    }
    pub fn show_progress(&self) -> bool { self.show_progress }
    pub fn page_size(&self) -> usize { self.page_size.rows() }
    pub fn list_left(&self) -> bool { self.list_position == ListPosition::Left }
    pub fn arrow_mode(&self) -> u8 { self.arrow_function.index() }
    pub fn seek13_seconds(&self) -> u32 { self.seek13_duration.seconds() }
}
