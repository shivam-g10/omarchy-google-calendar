import QtQuick
import Quickshell
import Quickshell.Io

// Shared calendar state for every bar instance. omarchy-shell creates it
// once because the manifest declares a keepLoaded service; bar widgets only
// retrieve it through shell.serviceFor("shivam.clock").
Item {
  id: root

  // Injected by omarchy-shell's service loader.
  property var shell: null

  readonly property string runtimeDir: Quickshell.env("XDG_RUNTIME_DIR") || ""
  readonly property string socketPath: runtimeDir === "" ? "" : runtimeDir + "/omarchy-calendar.sock"
  readonly property bool connected: calendarSocket.connected

  property string status: "connecting"
  property string error: ""
  property string lastSync: ""
  property int socketErrorCode: -1
  property bool refreshing: false
  property bool agendaLoading: false
  property bool addingAccount: false
  property bool cancellingAccount: false
  property bool oauthCancelable: false
  property bool oauthFinishing: false
  property string openingItemId: ""

  property var accounts: []
  property var items: []
  property date selectedDate: new Date()
  property string accountFilter: "all"
  readonly property string selectedAccountId: accountFilter
  property string agendaStart: ""
  property string agendaEnd: ""
  property string generatedAt: ""

  property int _nextRequestId: 0
  property var _pendingRequests: ({})
  property string _activeAgendaRequestId: ""
  property int _reconnectDelayMs: 1000
  property bool _destroying: false

  signal stateUpdated()
  signal agendaUpdated()
  signal requestFailed(string method, string code, string message)
  signal actionCompleted(string method, var result)
  signal backendConnected()
  signal backendDisconnected()

  function pad2(value) {
    return value < 10 ? "0" + value : String(value)
  }

  function localDate(value) {
    if (value instanceof Date) {
      if (!isNaN(value.getTime()))
        return new Date(value.getFullYear(), value.getMonth(), value.getDate())
      return null
    }

    var text = String(value || "")
    var match = /^(\d{4})-(\d{2})-(\d{2})/.exec(text)
    if (match) {
      var parsed = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]))
      if (parsed.getFullYear() === Number(match[1])
          && parsed.getMonth() === Number(match[2]) - 1
          && parsed.getDate() === Number(match[3])) return parsed
      return null
    }

    var fallback = new Date(value)
    if (isNaN(fallback.getTime())) return null
    return new Date(fallback.getFullYear(), fallback.getMonth(), fallback.getDate())
  }

  function dateKey(value) {
    var date = localDate(value)
    if (!date) return ""
    return date.getFullYear() + "-" + pad2(date.getMonth() + 1) + "-" + pad2(date.getDate())
  }

  function addLocalDays(value, days) {
    var date = localDate(value)
    if (!date) return null
    date.setDate(date.getDate() + days)
    return date
  }

  function monthRange(value) {
    var date = localDate(value) || new Date()
    // Seven-day shoulders cover any locale's six-week calendar grid.
    var start = new Date(date.getFullYear(), date.getMonth(), 1)
    start.setDate(start.getDate() - 7)
    var end = new Date(date.getFullYear(), date.getMonth() + 1, 1)
    end.setDate(end.getDate() + 7)
    return { start: dateKey(start), end: dateKey(end) }
  }

  function selectedRangeLoaded() {
    var key = dateKey(selectedDate)
    return key !== "" && agendaStart !== "" && agendaEnd !== ""
      && key >= agendaStart && key < agendaEnd
  }

  function selectDate(value) {
    var date = localDate(value)
    if (!date) return false
    selectedDate = date
    if (!selectedRangeLoaded()) {
      var range = monthRange(date)
      loadRange(range.start, range.end)
    }
    return true
  }

  function setAccountFilter(accountId) {
    var next = String(accountId || "all")
    if (next === "") next = "all"
    if (next === accountFilter) return
    accountFilter = next
    if (agendaStart === "" || agendaEnd === "") {
      var range = monthRange(selectedDate)
      loadRange(range.start, range.end)
    } else {
      requestAgenda()
    }
  }

  function setFilter(accountId) {
    setAccountFilter(accountId)
  }

  function loadRange(startIso, endIso) {
    var start = dateKey(startIso)
    var end = dateKey(endIso)
    if (start === "" || end === "" || start >= end) {
      requestFailed("get_agenda", "invalid_range", "Agenda range must use valid, increasing dates")
      return false
    }

    agendaStart = start
    agendaEnd = end
    return requestAgenda()
  }

  function setAgendaRange(startIso, endIso) {
    return loadRange(startIso, endIso)
  }

  function requestState() {
    return sendRequest("get_state", {}) !== ""
  }

  function requestAgenda() {
    if (agendaStart === "" || agendaEnd === "") {
      var range = monthRange(selectedDate)
      agendaStart = range.start
      agendaEnd = range.end
    }

    if (!calendarSocket.connected) {
      agendaLoading = false
      ensureConnected()
      return false
    }

    agendaLoading = true
    var requestId = sendRequest("get_agenda", {
      start: agendaStart,
      end: agendaEnd,
      account: accountFilter
    })
    _activeAgendaRequestId = requestId
    if (requestId === "") agendaLoading = false
    return requestId !== ""
  }

  function refresh() {
    if (!calendarSocket.connected) {
      ensureConnected()
      return false
    }
    if (refreshing) return true
    refreshing = true
    if (sendRequest("refresh", {}) === "") {
      refreshing = false
      return false
    }
    return true
  }

  function addAccount() {
    if (addingAccount) return false
    addingAccount = true
    cancellingAccount = false
    oauthCancelable = true
    oauthFinishing = false
    var requestId = sendRequest("add_account", {})
    if (requestId === "") {
      addingAccount = false
      oauthCancelable = false
      return false
    }
    return true
  }

  function cancelAddAccount() {
    if (!addingAccount || cancellingAccount || !oauthCancelable) return false
    cancellingAccount = true
    var requestId = sendRequest("cancel_add_account", {})
    if (requestId === "") {
      cancellingAccount = false
      return false
    }
    return true
  }

  function removeAccount(accountId) {
    var id = String(accountId || "")
    if (id === "") return false
    return sendRequest("remove_account", { accountId: id }) !== ""
  }

  function openItem(itemOrId) {
    var id = typeof itemOrId === "object" && itemOrId
      ? String(itemOrId.id || "")
      : String(itemOrId || "")
    if (id === "" || openingItemId !== "") return false
    openingItemId = id
    if (sendRequest("open_item", { itemId: id }) === "") {
      openingItemId = ""
      return false
    }
    return true
  }

  function copyObject(source) {
    var output = ({})
    for (var key in source) output[key] = source[key]
    return output
  }

  function sendRequest(method, params) {
    if (!calendarSocket.connected) {
      ensureConnected()
      requestFailed(method, "not_connected", "Calendar service is unavailable")
      return ""
    }

    _nextRequestId += 1
    var id = String(_nextRequestId)
    var pending = copyObject(_pendingRequests)
    pending[id] = {
      method: method,
      params: params || ({})
    }
    _pendingRequests = pending

    calendarSocket.write(JSON.stringify({
      id: id,
      method: method,
      params: params || ({})
    }) + "\n")
    calendarSocket.flush()
    return id
  }

  function takePending(id) {
    var key = String(id)
    var pending = _pendingRequests[key]
    if (!pending) return null

    var next = ({})
    for (var existing in _pendingRequests)
      if (existing !== key) next[existing] = _pendingRequests[existing]
    _pendingRequests = next
    return pending
  }

  function hasPendingMethod(method) {
    for (var id in _pendingRequests)
      if (_pendingRequests[id].method === method) return true
    return false
  }

  function applyState(state) {
    if (!state || typeof state !== "object") return
    status = String(state.status || "idle")
    error = state.error === undefined || state.error === null ? "" : String(state.error)
    lastSync = state.lastSync === undefined || state.lastSync === null ? "" : String(state.lastSync)
    accounts = Array.isArray(state.accounts) ? state.accounts : []

    var backendOAuth = state.oauthInProgress === true
    if (backendOAuth) {
      addingAccount = true
      oauthCancelable = state.oauthCancelable === true
      cancellingAccount = state.oauthCancelling === true
      oauthFinishing = state.oauthFinishing === true
    } else if (!hasPendingMethod("add_account")) {
      addingAccount = false
      cancellingAccount = false
      oauthCancelable = false
      oauthFinishing = false
    }

    if (accountFilter !== "all") {
      var found = false
      for (var i = 0; i < accounts.length; i++) {
        if (String(accounts[i].id || "") === accountFilter && accounts[i].enabled !== false) {
          found = true
          break
        }
      }
      if (!found) accountFilter = "all"
    }

    stateUpdated()
  }

  function applyAgenda(result, pending, responseId) {
    if (String(responseId) !== _activeAgendaRequestId) return
    if (!result) result = ({})
    var responseItems = Array.isArray(result) ? result : result.items
    if (!Array.isArray(responseItems)) responseItems = []

    var requestParams = pending && pending.params ? pending.params : ({})
    var responseStart = String(result.start || requestParams.start || "")
    var responseEnd = String(result.end || requestParams.end || "")
    var responseAccount = String(requestParams.account || "all")

    // A slow old request must never replace a newer range or account filter.
    if (responseStart !== agendaStart || responseEnd !== agendaEnd || responseAccount !== accountFilter) return

    items = responseItems
    generatedAt = result.generatedAt ? String(result.generatedAt) : ""
    if (String(responseId) === _activeAgendaRequestId) {
      agendaLoading = false
      _activeAgendaRequestId = ""
    }
    agendaUpdated()
  }

  function handleResponse(message) {
    var pending = takePending(message.id)
    if (!pending) return

    if (message.ok !== true) {
      var responseError = message.error && typeof message.error === "object" ? message.error : ({})
      var code = String(responseError.code || "request_failed")
      var text = String(responseError.message || "Calendar request failed")
      if (pending.method === "refresh") refreshing = false
      if (pending.method === "add_account") {
        addingAccount = false
        cancellingAccount = false
        oauthCancelable = false
        oauthFinishing = false
      }
      if (pending.method === "cancel_add_account") {
        cancellingAccount = false
        requestState()
      }
      if (pending.method === "open_item" && openingItemId === String(pending.params.itemId || ""))
        openingItemId = ""
      if (pending.method === "get_agenda" && String(message.id) === _activeAgendaRequestId) {
        agendaLoading = false
        _activeAgendaRequestId = ""
      }
      if (code !== "oauth_cancelled") {
        error = text
        requestFailed(pending.method, code, text)
      } else {
        error = ""
      }
      return
    }

    var result = message.result === undefined || message.result === null ? ({}) : message.result
    if (pending.method === "get_state") {
      applyState(result)
    } else if (pending.method === "get_agenda") {
      applyAgenda(result, pending, message.id)
    } else {
      if (pending.method === "add_account") {
        addingAccount = false
        cancellingAccount = false
        oauthCancelable = false
        oauthFinishing = false
      }
      if (pending.method === "cancel_add_account") {
        cancellingAccount = result.cancelled === true && hasPendingMethod("add_account")
        requestState()
      }
      if (pending.method === "open_item" && openingItemId === String(pending.params.itemId || ""))
        openingItemId = ""
      if (pending.method === "refresh") {
        refreshing = false
        requestState()
        requestAgenda()
      } else if (pending.method === "add_account" || pending.method === "remove_account") {
        requestState()
        requestAgenda()
      }
      actionCompleted(pending.method, result)
    }
  }

  function handleNotification(message) {
    var event = String(message.event || "")
    var data = message.data
    if (event === "state_changed") {
      applyState(data)
    } else if (event === "agenda_changed") {
      requestAgenda()
    }
  }

  function handleLine(line) {
    var text = String(line || "").trim()
    if (text === "") return

    var message
    try {
      message = JSON.parse(text)
    } catch (parseError) {
      error = "Calendar service returned invalid data"
      requestFailed("protocol", "invalid_json", error)
      return
    }

    if (message && message.id !== undefined) handleResponse(message)
    else if (message && message.event !== undefined) handleNotification(message)
  }

  function clearPending() {
    _pendingRequests = ({})
    _activeAgendaRequestId = ""
    refreshing = false
    agendaLoading = false
    addingAccount = false
    cancellingAccount = false
    oauthCancelable = false
    oauthFinishing = false
    openingItemId = ""
  }

  function ensureConnected() {
    if (_destroying || socketPath === "" || calendarSocket.connected || reconnectTimer.running) return
    calendarSocket.connected = true
  }

  function scheduleReconnect() {
    if (_destroying || socketPath === "" || reconnectTimer.running) return
    reconnectTimer.interval = _reconnectDelayMs
    reconnectTimer.restart()
    _reconnectDelayMs = Math.min(_reconnectDelayMs * 2, 30000)
  }

  Component.onCompleted: {
    if (socketPath === "") {
      status = "error"
      error = "XDG_RUNTIME_DIR is unavailable"
    } else {
      var range = monthRange(selectedDate)
      agendaStart = range.start
      agendaEnd = range.end
      Qt.callLater(root.ensureConnected)
    }
  }

  Component.onDestruction: {
    _destroying = true
    reconnectTimer.stop()
    calendarSocket.connected = false
  }

  Timer {
    id: reconnectTimer
    interval: 1000
    repeat: false
    onTriggered: root.ensureConnected()
  }

  Socket {
    id: calendarSocket
    path: root.socketPath
    connected: false

    parser: SplitParser {
      onRead: function(line) { root.handleLine(line) }
    }

    onConnectedChanged: {
      if (connected) {
        root._reconnectDelayMs = 1000
        root.socketErrorCode = -1
        root.error = ""
        root.status = "loading"
        root.backendConnected()
        root.requestState()
        root.requestAgenda()
      } else if (!root._destroying) {
        root.clearPending()
        root.status = "offline"
        root.error = "Calendar service is unavailable"
        root.backendDisconnected()
        root.scheduleReconnect()
      }
    }

    onError: {
      // Quickshell 0.3.1 exposes only an enum signal argument and no readable
      // error string. Keep diagnostics generic so QML tooling also remains
      // independent of Qt's private QLocalSocket enum registration.
      root.socketErrorCode = -1
      root.status = "offline"
      root.error = "Calendar service is unavailable"
      connected = false
      root.scheduleReconnect()
    }
  }
}
