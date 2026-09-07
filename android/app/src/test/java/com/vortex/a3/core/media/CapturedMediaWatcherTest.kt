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
}
