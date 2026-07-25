package com.vortex.a3.core.pairing

/**
 * Emoji SAS — render the 6-digit pairing SAS as 3 emoji + names.
 *
 * The handshake SAS is `value ∈ 0..1_000_000` (HMAC-derived, identical on both
 * devices). With EXACTLY 100 emoji, three base-100 digits cover 100³ =
 * 1_000_000 = the full SAS range, a clean bijection: the emoji carry the SAME
 * ~20 bits of MITM protection as the digits, just friendlier to compare.
 *
 * CRITICAL: this 100-entry table and the digit split MUST stay byte-identical to
 * the laptop side (linux/ui-tauri/src/lib/sasEmoji.ts) — a different order or
 * length makes the two devices show different emoji for the same SAS and every
 * pairing looks like a mismatch. Append only; never reorder.
 */
object SasEmoji {
    data class Glyph(val emoji: String, val name: String)

    /** 100 distinct, widely-rendered emoji. Index = base-100 digit. APPEND-ONLY. */
    val TABLE: List<Glyph> = listOf(
        Glyph("🦊", "Fox"), Glyph("🐼", "Panda"), Glyph("🦁", "Lion"), Glyph("🐯", "Tiger"),
        Glyph("🐶", "Dog"), Glyph("🐱", "Cat"), Glyph("🐵", "Monkey"), Glyph("🐸", "Frog"),
        Glyph("🐧", "Penguin"), Glyph("🦉", "Owl"), Glyph("🦅", "Eagle"), Glyph("🐝", "Bee"),
        Glyph("🦋", "Butterfly"), Glyph("🐢", "Turtle"), Glyph("🐙", "Octopus"), Glyph("🐬", "Dolphin"),
        Glyph("🐳", "Whale"), Glyph("🦈", "Shark"), Glyph("🐠", "Fish"), Glyph("🦀", "Crab"),
        Glyph("🐌", "Snail"), Glyph("🐞", "Ladybug"), Glyph("🦄", "Unicorn"), Glyph("🐴", "Horse"),
        Glyph("🐮", "Cow"), Glyph("🐷", "Pig"), Glyph("🐰", "Rabbit"), Glyph("🐨", "Koala"),
        Glyph("🐻", "Bear"), Glyph("🦒", "Giraffe"), Glyph("🐘", "Elephant"), Glyph("🦓", "Zebra"),
        Glyph("🦔", "Hedgehog"), Glyph("🦇", "Bat"), Glyph("🦜", "Parrot"), Glyph("🦚", "Peacock"),
        Glyph("🍎", "Apple"), Glyph("🍌", "Banana"), Glyph("🍓", "Strawberry"), Glyph("🍒", "Cherry"),
        Glyph("🍇", "Grapes"), Glyph("🍉", "Watermelon"), Glyph("🍑", "Peach"), Glyph("🍍", "Pineapple"),
        Glyph("🥝", "Kiwi"), Glyph("🥥", "Coconut"), Glyph("🌽", "Corn"), Glyph("🥕", "Carrot"),
        Glyph("🍄", "Mushroom"), Glyph("🌶️", "Pepper"), Glyph("🍕", "Pizza"), Glyph("🍔", "Burger"),
        Glyph("🌮", "Taco"), Glyph("🍩", "Donut"), Glyph("🍪", "Cookie"), Glyph("🎂", "Cake"),
        Glyph("🍦", "Ice cream"), Glyph("🍿", "Popcorn"), Glyph("☕", "Coffee"), Glyph("🍵", "Tea"),
        Glyph("⚽", "Soccer"), Glyph("🏀", "Basketball"), Glyph("🏈", "Football"), Glyph("🎾", "Tennis"),
        Glyph("🎱", "8-ball"), Glyph("🎯", "Target"), Glyph("🎲", "Dice"), Glyph("🎮", "Game"),
        Glyph("🎸", "Guitar"), Glyph("🎺", "Trumpet"), Glyph("🎻", "Violin"), Glyph("🥁", "Drum"),
        Glyph("🎹", "Piano"), Glyph("🎤", "Mic"), Glyph("🎧", "Headphones"), Glyph("🚗", "Car"),
        Glyph("🚀", "Rocket"), Glyph("✈️", "Plane"), Glyph("🚲", "Bike"), Glyph("⛵", "Sailboat"),
        Glyph("🚁", "Helicopter"), Glyph("🚂", "Train"), Glyph("⚓", "Anchor"), Glyph("🪂", "Parachute"),
        Glyph("🌙", "Moon"), Glyph("⭐", "Star"), Glyph("☀️", "Sun"), Glyph("⚡", "Lightning"),
        Glyph("🔥", "Fire"), Glyph("❄️", "Snowflake"), Glyph("🌈", "Rainbow"), Glyph("🌻", "Sunflower"),
        Glyph("🌹", "Rose"), Glyph("🌵", "Cactus"), Glyph("🌲", "Tree"), Glyph("🍀", "Clover"),
        Glyph("💎", "Diamond"), Glyph("🔑", "Key"), Glyph("🔔", "Bell"), Glyph("🎁", "Gift"),
    )

    /** Map the 6-digit SAS string (or value) to its 3 glyphs, most-significant
     *  digit first so it reads left→right like the number. Never throws. */
    fun glyphs(sas: String): List<Glyph> {
        val v = (sas.toIntOrNull() ?: 0).let { if (it < 0) 0 else it } % 1_000_000
        val a = (v / 10_000) % 100
        val b = (v / 100) % 100
        val c = v % 100
        return listOf(TABLE[a], TABLE[b], TABLE[c])
    }
}
