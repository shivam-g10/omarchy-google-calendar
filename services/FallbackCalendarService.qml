pragma Singleton
import ".." as Calendar

// Created only when a hosted widget cannot retrieve the shell-owned service.
// The QML engine retains this single client for the rest of its lifetime.
Calendar.CalendarService {}
