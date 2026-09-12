package com.vortex.a3.core.media

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Test

class CapturedMediaWatcherTest {
    @Test
    fun screenshots_and_camera_buckets_are_recognised() {
        assertEquals(CapturedKind.SCREENSHOT, classifyCapture("Screenshots", "DCIM/Screenshots/"))
        assertEquals(CapturedKind.PHOTO, classifyCapture("Camera", "DCIM/Camera/"))
    }

    @Test
    fun the_path_decides_when_the_bucket_label_is_localised() {
        assertEquals(CapturedKind.SCREENSHOT, classifyCapture("Скриншоты", "DCIM/Screenshots/"))
        assertEquals(CapturedKind.PHOTO, classifyCapture(null, "DCIM/Camera"))
    }

    @Test
    fun other_folders_are_not_ours() {
        assertNull(classifyCapture("Download", "Download/"))
        assertNull(classifyCapture("WhatsApp Images", "Pictures/WhatsApp/WhatsApp Images/"))
        assertNull(classifyCapture("Camera Roll", "Pictures/Camera Roll/"))
        assertNull(classifyCapture(null, null))
    }

    @Test
    fun recorder_folders_and_camera_clips_are_recognised() {
        assertEquals(
            CapturedKind.SCREEN_RECORDING,
            classifyVideoCapture("ScreenRecorder", "DCIM/ScreenRecorder/"),
        )
        assertEquals(
            CapturedKind.SCREEN_RECORDING,
            classifyVideoCapture("Записи экрана", "DCIM/ScreenRecorder/"),
        )
        assertEquals(CapturedKind.VIDEO, classifyVideoCapture("Camera", "DCIM/Camera/"))
    }

    // `Movies/` is where stock Android records, and also where downloaded
    // films live. It is left unmatched on purpose — see classifyVideoCapture.
    @Test
    fun movies_is_not_treated_as_a_recording_folder() {
        assertNull(classifyVideoCapture("Movies", "Movies/"))
        assertNull(classifyVideoCapture("Telegram", "Movies/Telegram/"))
        assertNull(classifyVideoCapture("Download", "Download/"))
        assertNull(classifyVideoCapture(null, null))
    }

    // The two collections are scanned separately, so the image classifier
    // must never claim a video folder and vice versa.
    @Test
    fun the_two_classifiers_do_not_claim_each_others_folders() {
        assertNull(classifyCapture("ScreenRecorder", "DCIM/ScreenRecorder/"))
        // "Camera" holds both stills and clips; each collection only ever
        // shows its own rows, so both classifying it is correct.
        assertEquals(CapturedKind.PHOTO, classifyCapture("Camera", "DCIM/Camera/"))
        assertEquals(CapturedKind.VIDEO, classifyVideoCapture("Camera", "DCIM/Camera/"))
    }
}
