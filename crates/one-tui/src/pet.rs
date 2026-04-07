use std::time::Instant;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

/// A small companion character that lives in the TUI.
/// Has moods, occasional speech bubbles, and reacts to events.
pub struct Pet {
    pub name: String,
    pub species: Species,
    pub mood: Mood,
    pub enabled: bool,
    speech: Option<SpeechBubble>,
    last_event: Instant,
    idle_since: Instant,
}

struct SpeechBubble {
    text: String,
    expires: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Species {
    Duck,
    Cat,
    Dog,
    Fox,
    Crab,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    Happy,
    Thinking,
    Nervous,
    Sleeping,
    Excited,
    Neutral,
}

impl Pet {
    pub fn new(name: String, species_str: &str, enabled: bool) -> Self {
        let species = match species_str {
            "cat" => Species::Cat,
            "dog" => Species::Dog,
            "fox" => Species::Fox,
            "crab" => Species::Crab,
            _ => Species::Duck,
        };

        Self {
            name,
            species,
            mood: Mood::Neutral,
            enabled,
            speech: None,
            last_event: Instant::now(),
            idle_since: Instant::now(),
        }
    }

    pub fn from_config(config: &one_core::config::PetConfig) -> Self {
        Self::new(config.name.clone(), &config.species, config.enabled)
    }

    /// Trigger a mood change and optional speech bubble
    pub fn on_user_message(&mut self) {
        self.mood = Mood::Thinking;
        self.last_event = Instant::now();
        self.idle_since = Instant::now();
    }

    pub fn on_response_start(&mut self) {
        self.mood = Mood::Thinking;
        self.last_event = Instant::now();
    }

    pub fn on_response_complete(&mut self) {
        self.mood = Mood::Happy;
        self.last_event = Instant::now();
        self.idle_since = Instant::now();

        // Occasional happy comment
        if fastrand::u8(..).is_multiple_of(5) {
            self.say(self.random_happy_message(), 4);
        }
    }

    pub fn on_error(&mut self) {
        self.mood = Mood::Nervous;
        self.last_event = Instant::now();
        self.say(self.random_error_message(), 5);
    }

    pub fn on_tool_call(&mut self, tool_name: &str) {
        self.mood = Mood::Excited;
        self.last_event = Instant::now();

        if fastrand::u8(..).is_multiple_of(3) {
            let msg = match tool_name {
                "bash" => "running something...",
                "file_read" => "reading...",
                "file_write" => "writing!",
                "file_edit" => "editing...",
                "grep" => "searching...",
                _ => "working...",
            };
            self.say(msg.to_string(), 3);
        }
    }

    /// Update mood based on idle time
    pub fn tick(&mut self) {
        let idle_secs = self.idle_since.elapsed().as_secs();

        if idle_secs > 120 && self.mood != Mood::Sleeping {
            self.mood = Mood::Sleeping;
            self.say("zzz...".to_string(), 10);
        } else if idle_secs > 30 && self.mood == Mood::Happy {
            self.mood = Mood::Neutral;
        }

        // Clear expired speech
        if let Some(ref bubble) = self.speech
            && Instant::now() > bubble.expires
        {
            self.speech = None;
        }
    }

    fn say(&mut self, text: String, duration_secs: u64) {
        self.speech = Some(SpeechBubble {
            text,
            expires: Instant::now() + std::time::Duration::from_secs(duration_secs),
        });
    }

    pub fn ascii_art(&self) -> &str {
        match (self.species, self.mood) {
            (Species::Duck, Mood::Happy) => ">o)  ~",
            (Species::Duck, Mood::Thinking) => ">o)  ?",
            (Species::Duck, Mood::Nervous) => ">o) !!",
            (Species::Duck, Mood::Sleeping) => "-o)  z",
            (Species::Duck, Mood::Excited) => ">o)  !",
            (Species::Duck, Mood::Neutral) => ">o)   ",

            (Species::Cat, Mood::Happy) => "=^.^=",
            (Species::Cat, Mood::Thinking) => "=^.o=",
            (Species::Cat, Mood::Nervous) => "=O.O=",
            (Species::Cat, Mood::Sleeping) => "=^-^=",
            (Species::Cat, Mood::Excited) => "=^!^=",
            (Species::Cat, Mood::Neutral) => "=^.^=",

            (Species::Dog, Mood::Happy) => "U^ェ^U",
            (Species::Dog, Mood::Thinking) => "U-ェ-U",
            (Species::Dog, Mood::Nervous) => "U;ェ;U",
            (Species::Dog, Mood::Sleeping) => "U-.-U",
            (Species::Dog, Mood::Excited) => "U!ェ!U",
            (Species::Dog, Mood::Neutral) => "U・ェ・U",

            (Species::Fox, Mood::Happy) => "^ↀᴥↀ^",
            (Species::Fox, Mood::Thinking) => "^-ᴥ-^",
            (Species::Fox, Mood::Nervous) => "^°ᴥ°^",
            (Species::Fox, Mood::Sleeping) => "^=ᴥ=^",
            (Species::Fox, Mood::Excited) => "^!ᴥ!^",
            (Species::Fox, Mood::Neutral) => "^·ᴥ·^",

            (Species::Crab, Mood::Happy) => "(\\/)!_!(\\/) ",
            (Species::Crab, Mood::Thinking) => "(\\/)?_?(\\/)",
            (Species::Crab, Mood::Nervous) => "(\\/);_;(\\/)",
            (Species::Crab, Mood::Sleeping) => "(\\/)-_-(\\/)",
            (Species::Crab, Mood::Excited) => "(\\/)*_*(\\/)",
            (Species::Crab, Mood::Neutral) => "(\\/)'_'(\\/)",
        }
    }

    fn random_happy_message(&self) -> String {
        let messages = [
            "nice!",
            "looks good!",
            "smooth.",
            "nailed it!",
            "great work!",
            "clean code!",
        ];
        messages[fastrand::usize(..messages.len())].to_string()
    }

    fn random_error_message(&self) -> String {
        let messages = ["uh oh...", "hmm...", "that's odd", "we'll fix it!", "oops!"];
        messages[fastrand::usize(..messages.len())].to_string()
    }

    /// Render the pet into the status bar area
    pub fn render(&self, f: &mut Frame, area: Rect) {
        if !self.enabled {
            return;
        }

        let mut spans = vec![
            Span::styled(self.ascii_art(), Style::default().fg(self.mood_color())),
            Span::styled(
                format!(" {}", self.name),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ),
        ];

        if let Some(ref bubble) = self.speech {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("\"{}\"", bubble.text),
                Style::default().fg(Color::Yellow),
            ));
        }

        let widget = Paragraph::new(Line::from(spans));
        f.render_widget(widget, area);
    }

    pub fn mood_color(&self) -> Color {
        match self.mood {
            Mood::Happy => Color::Green,
            Mood::Thinking => Color::Cyan,
            Mood::Nervous => Color::Red,
            Mood::Sleeping => Color::DarkGray,
            Mood::Excited => Color::Magenta,
            Mood::Neutral => Color::White,
        }
    }
}
