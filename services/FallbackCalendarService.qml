pragma Singleton
import ".." as Calendar

// Created only for bar facades without access to the shell-owned service.
// The QML engine retains this single client for the rest of its lifetime.
Calendar.CalendarService {}
