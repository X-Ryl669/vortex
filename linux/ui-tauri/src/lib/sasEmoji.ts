// Emoji SAS — render the 6-digit pairing SAS as 3 emoji + names.
//
// The handshake SAS is `value ∈ 0..1_000_000` (HMAC-derived, identical on both
// devices — see daemon core::crypto::sas). With EXACTLY 100 emoji, three
// base-100 digits cover 100³ = 1_000_000 = the full SAS range, a clean
// bijection: the emoji carry the SAME ~20 bits of MITM protection as the digits,
// just friendlier to compare across two screens.
//
// CRITICAL: this 100-entry table and the digit split MUST stay byte-identical to
// the Android side (com.vortex.a3.core.pairing.SasEmoji) — a different order or
// length would make the two devices show different emoji for the same SAS and
// every pairing would look like a mismatch. Append only; never reorder.

export interface SasGlyph {
  emoji: string;
  name: string;
}

/** 100 distinct, widely-rendered emoji. Index = base-100 digit. APPEND-ONLY. */
export const SAS_EMOJI: SasGlyph[] = [
  { emoji: "🦊", name: "Fox" },        { emoji: "🐼", name: "Panda" },
  { emoji: "🦁", name: "Lion" },       { emoji: "🐯", name: "Tiger" },
  { emoji: "🐶", name: "Dog" },        { emoji: "🐱", name: "Cat" },
  { emoji: "🐵", name: "Monkey" },     { emoji: "🐸", name: "Frog" },
  { emoji: "🐧", name: "Penguin" },    { emoji: "🦉", name: "Owl" },
  { emoji: "🦅", name: "Eagle" },      { emoji: "🐝", name: "Bee" },
  { emoji: "🦋", name: "Butterfly" },  { emoji: "🐢", name: "Turtle" },
  { emoji: "🐙", name: "Octopus" },    { emoji: "🐬", name: "Dolphin" },
  { emoji: "🐳", name: "Whale" },      { emoji: "🦈", name: "Shark" },
  { emoji: "🐠", name: "Fish" },       { emoji: "🦀", name: "Crab" },
  { emoji: "🐌", name: "Snail" },      { emoji: "🐞", name: "Ladybug" },
  { emoji: "🦄", name: "Unicorn" },    { emoji: "🐴", name: "Horse" },
  { emoji: "🐮", name: "Cow" },        { emoji: "🐷", name: "Pig" },
  { emoji: "🐰", name: "Rabbit" },     { emoji: "🐨", name: "Koala" },
  { emoji: "🐻", name: "Bear" },       { emoji: "🦒", name: "Giraffe" },
  { emoji: "🐘", name: "Elephant" },   { emoji: "🦓", name: "Zebra" },
  { emoji: "🦔", name: "Hedgehog" },   { emoji: "🦇", name: "Bat" },
  { emoji: "🦜", name: "Parrot" },     { emoji: "🦚", name: "Peacock" },
  { emoji: "🍎", name: "Apple" },      { emoji: "🍌", name: "Banana" },
  { emoji: "🍓", name: "Strawberry" }, { emoji: "🍒", name: "Cherry" },
  { emoji: "🍇", name: "Grapes" },     { emoji: "🍉", name: "Watermelon" },
  { emoji: "🍑", name: "Peach" },      { emoji: "🍍", name: "Pineapple" },
  { emoji: "🥝", name: "Kiwi" },       { emoji: "🥥", name: "Coconut" },
  { emoji: "🌽", name: "Corn" },       { emoji: "🥕", name: "Carrot" },
  { emoji: "🍄", name: "Mushroom" },   { emoji: "🌶️", name: "Pepper" },
  { emoji: "🍕", name: "Pizza" },      { emoji: "🍔", name: "Burger" },
  { emoji: "🌮", name: "Taco" },       { emoji: "🍩", name: "Donut" },
  { emoji: "🍪", name: "Cookie" },     { emoji: "🎂", name: "Cake" },
  { emoji: "🍦", name: "Ice cream" },  { emoji: "🍿", name: "Popcorn" },
  { emoji: "☕", name: "Coffee" },     { emoji: "🍵", name: "Tea" },
  { emoji: "⚽", name: "Soccer" },     { emoji: "🏀", name: "Basketball" },
  { emoji: "🏈", name: "Football" },   { emoji: "🎾", name: "Tennis" },
  { emoji: "🎱", name: "8-ball" },     { emoji: "🎯", name: "Target" },
  { emoji: "🎲", name: "Dice" },       { emoji: "🎮", name: "Game" },
  { emoji: "🎸", name: "Guitar" },     { emoji: "🎺", name: "Trumpet" },
  { emoji: "🎻", name: "Violin" },     { emoji: "🥁", name: "Drum" },
  { emoji: "🎹", name: "Piano" },      { emoji: "🎤", name: "Mic" },
  { emoji: "🎧", name: "Headphones" }, { emoji: "🚗", name: "Car" },
  { emoji: "🚀", name: "Rocket" },     { emoji: "✈️", name: "Plane" },
  { emoji: "🚲", name: "Bike" },       { emoji: "⛵", name: "Sailboat" },
  { emoji: "🚁", name: "Helicopter" }, { emoji: "🚂", name: "Train" },
  { emoji: "⚓", name: "Anchor" },     { emoji: "🪂", name: "Parachute" },
  { emoji: "🌙", name: "Moon" },       { emoji: "⭐", name: "Star" },
  { emoji: "☀️", name: "Sun" },        { emoji: "⚡", name: "Lightning" },
  { emoji: "🔥", name: "Fire" },       { emoji: "❄️", name: "Snowflake" },
  { emoji: "🌈", name: "Rainbow" },    { emoji: "🌻", name: "Sunflower" },
  { emoji: "🌹", name: "Rose" },       { emoji: "🌵", name: "Cactus" },
  { emoji: "🌲", name: "Tree" },       { emoji: "🍀", name: "Clover" },
  { emoji: "💎", name: "Diamond" },    { emoji: "🔑", name: "Key" },
  { emoji: "🔔", name: "Bell" },       { emoji: "🎁", name: "Gift" },
];

/** Map the 6-digit SAS (string or value 0..999999) to its 3 glyphs.
 *  digit order = most-significant first, so it reads left→right like the number.
 *  Out-of-range / unparsable input falls back to the first glyph (never throws). */
export function sasToGlyphs(sas: string | number): SasGlyph[] {
  let v = typeof sas === "number" ? sas : parseInt(sas, 10);
  if (!Number.isFinite(v) || v < 0) v = 0;
  v %= 1_000_000;
  const a = Math.floor(v / 10_000) % 100;
  const b = Math.floor(v / 100) % 100;
  const c = v % 100;
  return [SAS_EMOJI[a], SAS_EMOJI[b], SAS_EMOJI[c]];
}
