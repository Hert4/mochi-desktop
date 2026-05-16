#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PetMood {
    Idle,
    Thinking,
    Happy,
    Sad,
    Sleeping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PetCharacter {
    Mochi,
    Bunny,
    Frog,
    Robot,
    Dragon,
}

impl PetCharacter {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "mochi" | "cat" => Some(Self::Mochi),
            "bunny" | "rabbit" => Some(Self::Bunny),
            "frog" => Some(Self::Frog),
            "robot" | "bot" => Some(Self::Robot),
            "dragon" => Some(Self::Dragon),
            _ => None,
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Mochi => "mochi",
            Self::Bunny => "bunny",
            Self::Frog => "frog",
            Self::Robot => "robot",
            Self::Dragon => "dragon",
        }
    }

    #[must_use]
    pub fn all() -> &'static [Self] {
        &[Self::Mochi, Self::Bunny, Self::Frog, Self::Robot, Self::Dragon]
    }
}

#[must_use]
pub fn sprite(mood: PetMood) -> &'static str {
    sprite_for(PetCharacter::Mochi, mood)
}

#[must_use]
#[allow(clippy::too_many_lines)]
pub fn sprite_for(character: PetCharacter, mood: PetMood) -> &'static str {
    match (character, mood) {
        (PetCharacter::Mochi, PetMood::Idle) => {
            "  ／l、     \n\
             ﾞ（ﾟ､ ｡ ７\n\
              l、ﾞ ~ヽ  "
        }
        (PetCharacter::Mochi, PetMood::Thinking) => {
            "  ／l、 ?   \n\
             ﾞ（- ､ -７\n\
              l、ﾞ ~ヽ  "
        }
        (PetCharacter::Mochi, PetMood::Happy) => {
            "  ／l、 ♪  \n\
             ﾞ（=^･^=)\n\
              l、ﾞ ~ヽ  "
        }
        (PetCharacter::Mochi, PetMood::Sad) => {
            "  ／l、     \n\
             ﾞ（T ､ T７\n\
              l、ﾞ ~ヽ  "
        }
        (PetCharacter::Mochi, PetMood::Sleeping) => {
            "  ／l、 zZ \n\
             ﾞ（-  -７\n\
              l、ﾞ ~ヽ  "
        }

        (PetCharacter::Bunny, PetMood::Idle) => {
            " (\\(\\   \n\
             (•ᴥ•)  \n\
             c(\")(\") "
        }
        (PetCharacter::Bunny, PetMood::Thinking) => {
            " (\\(\\   \n\
             (-.-) ?\n\
             c(\")(\") "
        }
        (PetCharacter::Bunny, PetMood::Happy) => {
            " (\\(\\   \n\
             (^ᴥ^)  ♪\n\
             c(\")(\") "
        }
        (PetCharacter::Bunny, PetMood::Sad) => {
            " (\\(\\   \n\
             (T_T)  \n\
             c(\")(\") "
        }
        (PetCharacter::Bunny, PetMood::Sleeping) => {
            " (\\(\\   zZ\n\
             (-.-)  \n\
             c(\")(\") "
        }

        (PetCharacter::Frog, PetMood::Idle) => {
            " (.)(.) \n\
             (.|.)  \n\
              `-´   "
        }
        (PetCharacter::Frog, PetMood::Thinking) => {
            " (-)(-) ?\n\
             (.|.)  \n\
              `-´   "
        }
        (PetCharacter::Frog, PetMood::Happy) => {
            " (^)(^) ♪\n\
             (.|.)  \n\
              \\_/   "
        }
        (PetCharacter::Frog, PetMood::Sad) => {
            " (T)(T) \n\
             (.|.)  \n\
              ___   "
        }
        (PetCharacter::Frog, PetMood::Sleeping) => {
            " (-)(-) zZ\n\
             (.|.)  \n\
              `-´   "
        }

        (PetCharacter::Robot, PetMood::Idle) => {
            " ┌[¯]┐ \n\
             |o_o|\n\
             |___| "
        }
        (PetCharacter::Robot, PetMood::Thinking) => {
            " ┌[¯]┐ ?\n\
             |⊙_⊙|\n\
             |___| "
        }
        (PetCharacter::Robot, PetMood::Happy) => {
            " ┌[¯]┐ ♪\n\
             |^_^|\n\
             |___| "
        }
        (PetCharacter::Robot, PetMood::Sad) => {
            " ┌[¯]┐ \n\
             |x_x|\n\
             |___| "
        }
        (PetCharacter::Robot, PetMood::Sleeping) => {
            " ┌[¯]┐ zZ\n\
             |-_-|\n\
             |___| "
        }

        (PetCharacter::Dragon, PetMood::Idle) => {
            "  /\\___/\\  \n\
              ( o.o )\n\
               > ^ <  "
        }
        (PetCharacter::Dragon, PetMood::Thinking) => {
            "  /\\___/\\  ?\n\
              ( -.- )\n\
               > ^ <  "
        }
        (PetCharacter::Dragon, PetMood::Happy) => {
            "  /\\___/\\  ♪\n\
              ( ^.^ )~\n\
               > w <  "
        }
        (PetCharacter::Dragon, PetMood::Sad) => {
            "  /\\___/\\  \n\
              ( T.T )\n\
               > _ <  "
        }
        (PetCharacter::Dragon, PetMood::Sleeping) => {
            "  /\\___/\\  zZ\n\
              ( -.- )\n\
               > z <  "
        }
    }
}

#[must_use]
pub fn label(mood: PetMood) -> &'static str {
    match mood {
        PetMood::Idle => "idle",
        PetMood::Thinking => "thinking...",
        PetMood::Happy => "happy",
        PetMood::Sad => "sad",
        PetMood::Sleeping => "zzz",
    }
}

/// Multi-line welcome banner for a given character.
#[must_use]
pub fn banner_for(character: PetCharacter) -> Vec<String> {
    let pet = sprite_for(character, PetMood::Happy);
    let mut lines: Vec<String> = pet.lines().map(str::to_owned).collect();
    lines.push(String::new());
    lines.push(format!("  ~ Mochi: your local-first terminal pet ({}) ~", character.name()));
    lines
}

#[cfg(test)]
mod tests {
    use super::{PetCharacter, PetMood, banner_for, label, sprite, sprite_for};

    #[test]
    fn every_character_renders_every_mood() {
        for ch in PetCharacter::all() {
            for m in
                [PetMood::Idle, PetMood::Thinking, PetMood::Happy, PetMood::Sad, PetMood::Sleeping]
            {
                let s = sprite_for(*ch, m);
                assert!(s.lines().count() >= 3, "{ch:?}/{m:?} sprite too short");
            }
        }
    }

    #[test]
    fn default_sprite_matches_mochi() {
        assert_eq!(sprite(PetMood::Idle), sprite_for(PetCharacter::Mochi, PetMood::Idle));
    }

    #[test]
    fn parse_accepts_aliases() {
        assert_eq!(PetCharacter::parse("cat"), Some(PetCharacter::Mochi));
        assert_eq!(PetCharacter::parse("rabbit"), Some(PetCharacter::Bunny));
        assert_eq!(PetCharacter::parse("bot"), Some(PetCharacter::Robot));
        assert_eq!(PetCharacter::parse("nope"), None);
    }

    #[test]
    fn labels_are_non_empty() {
        for m in [PetMood::Idle, PetMood::Thinking, PetMood::Happy, PetMood::Sad, PetMood::Sleeping]
        {
            assert!(!label(m).is_empty());
        }
    }

    #[test]
    fn banner_includes_character_name() {
        let lines = banner_for(PetCharacter::Bunny);
        assert!(lines.iter().any(|line| line.contains("bunny")));
    }
}
