// English source strings are the fallback. Add language catalogs here without
// changing the shared service or the user's calendar content.
var spanish = {
  "Calendar": "Calendario",
  "Refresh calendars": "Actualizar calendarios",
  "Add Google account": "Añadir cuenta de Google",
  "Cancel Google sign-in": "Cancelar inicio de sesión de Google",
  "Remove selected account": "Quitar cuenta seleccionada",
  "All accounts": "Todas las cuentas",
  "Account": "Cuenta",
  "Google account": "cuenta de Google",
  "Previous month": "Mes anterior",
  "Next month": "Mes siguiente",
  "Back to today": "Volver a hoy",
  "Start weeks on {day}": "Empezar las semanas el {day}",
  "Nothing scheduled": "No hay nada programado",
  "Calendar service unavailable": "Servicio de calendario no disponible",
  "Calendar service is unavailable": "El servicio de calendario no está disponible",
  "Connecting to calendar…": "Conectando con el calendario…",
  "Google OAuth setup needed": "Se requiere configurar OAuth de Google",
  "Google setup required": "Se requiere configurar Google",
  "No accounts connected": "No hay cuentas conectadas",
  "Connect a Google account": "Conecta una cuenta de Google",
  "connecting…": "conectando…",
  "syncing…": "sincronizando…",
  "synced": "sincronizado",
  "cached offline": "caché sin conexión",
  "sync issue": "problema de sincronización",
  "Cancelling Google sign-in…": "Cancelando el inicio de sesión de Google…",
  "Reconnecting Google account…": "Reconectando la cuenta de Google…",
  "Finishing Google sign-in…": "Finalizando el inicio de sesión de Google…",
  "Waiting for Google sign-in…": "Esperando el inicio de sesión de Google…",
  "Opening Google sign-in…": "Abriendo el inicio de sesión de Google…",
  "Opening event…": "Abriendo evento…",
  "Complete sign-in": "Completar inicio de sesión",
  "Sign in again →": "Volver a iniciar sesión →",
  "Complete Google sign-in for {account} in your browser.": "Completa el inicio de sesión de Google para {account} en tu navegador.",
  "Google access expired for {account}.": "El acceso a Google de {account} ha caducado.",
  "Google access expired for {count} account. Select an affected account to sign in again.": "El acceso a Google de {count} cuenta ha caducado. Selecciona la cuenta afectada para volver a iniciar sesión.",
  "Google access expired for {count} accounts. Select an affected account to sign in again.": "El acceso a Google de {count} cuentas ha caducado. Selecciona una cuenta afectada para volver a iniciar sesión.",
  "{count} account": "{count} cuenta",
  "{count} accounts": "{count} cuentas",
  "{count} item": "{count} elemento",
  "{count} items": "{count} elementos",
  "{weekday}, {day} {month}": "{weekday}, {day} de {month}",
  "Untitled": "Sin título",
  "Due today": "Vence hoy",
  "All day": "Todo el día",
  "Google Tasks · date only": "Google Tasks · solo fecha",
  "Tasks are date-only": "Las tareas solo tienen fecha",
  "Cached offline · Tasks are date-only": "Caché sin conexión · Las tareas solo tienen fecha",
  "BORN": "NACÍ EN",
  "LIVE TO": "VIVIR HASTA",
  "LIFE": "VIDA",
  "Memento Mori": "Recuerda que morirás",
  "year": "año",
  "W": "S",
  "At start": "Al comenzar",
  "{offset} before": "{offset} antes",
  "Calendar request failed": "La solicitud al calendario falló",
  "Calendar service returned invalid data": "El servicio de calendario devolvió datos no válidos",
  "Agenda range must use valid, increasing dates": "El rango de la agenda debe usar fechas válidas en orden creciente",
  "XDG_RUNTIME_DIR is unavailable": "XDG_RUNTIME_DIR no está disponible"
}

function localeName(configured, systemLocale) {
  var name = String(configured || "").trim() || String(systemLocale || "en_US")
  name = name.replace(/-/g, "_").replace(/[.@].*$/, "")
  return name === "C" || name === "POSIX" || name === "" ? "en_US" : name
}

function translate(locale, source, values) {
  var text = String(source || "")
  var language = String(locale || "en").split(/[-_]/)[0].toLowerCase()
  if (language === "es" && Object.prototype.hasOwnProperty.call(spanish, text)) text = spanish[text]
  return text.replace(/\{([^{}]+)\}/g, function(match, key) {
    return values && values[key] !== undefined ? String(values[key]) : match
  })
}

if (typeof module !== "undefined") module.exports = { localeName: localeName, translate: translate }
