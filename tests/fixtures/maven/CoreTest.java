package demo;

import static org.junit.jupiter.api.Assertions.*;

import org.junit.jupiter.api.Disabled;
import org.junit.jupiter.api.Test;

class CoreTest {
    @Test
    void adds() {
        assertEquals(4, 2 + 2);
    }

    @Test
    void subtracts() {
        assertEquals(1, 3 - 1, "subtraction is off");
    }

    @Test
    void throwsUnexpectedly() {
        throw new IllegalStateException("boom");
    }

    @Disabled("not ready")
    @Test
    void divides() {
    }

    @Test
    void printsThenFails() {
        System.out.println("hello from app test");
        assertTrue(false);
    }
}
