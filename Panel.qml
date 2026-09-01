import QtQuick
import Quickshell
import qs.Commons
import qs.Ui
import "Model.js" as Model

// Multi-account calendar popup. QML owns presentation only; an injected
// service owns OAuth, secrets, synchronization, and the private event cache.
Panel {
  id: root
  moduleName: hostWidget && hostWidget.moduleName ? String(hostWidget.moduleName) : "shivam.clock"
  ipcTarget: moduleName
  manageIpc: false

  property var anchorItem: null
  property var service: null
  property var calendarService: null
  readonly property var actualService: calendarService || service

  // The bar tracks the widget mounted in its slot — BarWidget.qml — not this
  // nested panel. Everything the bar identifies a panel by has to be that
  // widget: the popout coordinator (and with it the open-panel dot under the
  // pill) compares against `slot.activeItem`, and switchPanelFrom looks the
  // slot up the same way.
  property var hostWidget: null
  readonly property var barIdentity: hostWidget || root

  // ---- Today. SystemClock keeps this honest across midnight so the
  //      highlight rolls over without the panel being reopened.
  property date today: new Date()
  readonly property string todayKey: Model.keyForDate(today)

  property int viewYear: today.getFullYear()
  property int viewMonth: today.getMonth()
  property string selectedDateKey: todayKey
  property string requestedRange: ""

  readonly property date viewDate: new Date(viewYear, viewMonth, 1)
  readonly property bool viewingCurrentMonth: viewYear === today.getFullYear() && viewMonth === today.getMonth()

  // Pinned to today, not to the month being browsed — stepping through the
  // calendar does not change how much of the year is gone.
  readonly property real yearDone: Model.yearProgress(today.getFullYear(), today.getMonth(), today.getDate())
  readonly property int yearDonePercent: Model.yearProgressPercent(today.getFullYear(), today.getMonth(), today.getDate())

  // Memento mori, for anyone who goes looking: double-tapping the year bar
  // asks for a birth year and a life expectancy, and a second bar tracks one
  // against the other. A birth year rather than an age, so it keeps counting
  // on its own. Without one the bar stays hidden.
  readonly property int birthYear: Model.parseBirthYear(setting("birthYear", 0), today.getFullYear())
  readonly property int age: Model.ageFromBirthYear(birthYear, today.getFullYear())
  readonly property int lifeExpectancy: Model.parseLifeExpectancy(setting("lifeExpectancy", 0))
  readonly property real lifeDone: Model.lifeProgress(age, lifeExpectancy)
  readonly property int lifeDonePercent: Model.lifeProgressPercent(age, lifeExpectancy)
  property bool editingLife: false

  // Unset falls through to the locale's own first day, so a fresh install
  // starts out matching the rest of the desktop rather than a hardcoded
  // convention. Clicking the grid's "W" heading writes the choice back to
  // shell.json.
  readonly property int weekStart: Model.normalizedWeekStart(setting("weekStartDay", null), Qt.locale().firstDayOfWeek)
  // The interface is English throughout, so day names are not taken from the
  // system locale. Where the week starts still is: that is a regional
  // convention rather than a translation, and it stays overridable above.
  readonly property var labelLocale: Qt.locale("en_US")
  readonly property string nextWeekStartLabel: labelLocale.dayName(Model.toggledWeekStart(weekStart), Locale.LongFormat)
  readonly property var weekdays: Model.weekdayOrder(weekStart)
  readonly property var weeks: Model.monthGrid(viewYear, viewMonth, weekStart, todayKey)

  readonly property var serviceAccounts: actualService && actualService.accounts ? actualService.accounts : []
  readonly property var serviceItems: actualService && actualService.items ? actualService.items : []
  readonly property string serviceStatus: actualService && actualService.status !== undefined
    ? String(actualService.status)
    : "unavailable"
  readonly property string serviceError: actualService && actualService.error !== undefined
    ? String(actualService.error || "")
    : ""
  readonly property bool backendConnected: !!actualService && actualService.connected === true
  readonly property bool addingAccount: !!actualService && actualService.addingAccount === true
  readonly property bool cancellingAccount: !!actualService && actualService.cancellingAccount === true
  readonly property bool oauthCancelable: !!actualService && actualService.oauthCancelable === true
  readonly property bool oauthFinishing: !!actualService && actualService.oauthFinishing === true
  readonly property string openingItemId: actualService && actualService.openingItemId !== undefined
    ? String(actualService.openingItemId || "")
    : ""
  readonly property bool serviceBusy: !!actualService
    && (actualService.refreshing === true || actualService.agendaLoading === true || serviceStatus === "syncing")
  readonly property string accountFilter: actualService && actualService.accountFilter !== undefined
    ? String(actualService.accountFilter || "all")
    : (actualService && actualService.selectedAccountId !== undefined
      ? String(actualService.selectedAccountId || "all")
      : "all")
  readonly property var agendaItems: Model.agendaItemsForDay(serviceItems, selectedDateKey, accountFilter)
  readonly property date selectedDate: dateFromKey(selectedDateKey)


  // Guarded so the widget renders before the bar is injected (the bar-widget
  // contract instantiates it bare).
  readonly property color contentForeground: bar ? bar.foreground : Color.foreground
  readonly property string contentFontFamily: bar ? bar.fontFamily : Style.font.family

  readonly property int cellWidth: Style.space(52)
  readonly property int cellHeight: Style.space(34)
  readonly property int cellSpacing: Style.space(2)
  readonly property int weekColumnWidth: Style.space(32)
  readonly property int gutterWidth: Style.space(14)

  readonly property bool wideLayout: panel.width >= Style.space(700)

  function dateFromKey(key) {
    var parts = String(key || "").split("-")
    if (parts.length !== 3) return new Date(root.today.getFullYear(), root.today.getMonth(), root.today.getDate())
    return new Date(Number(parts[0]), Number(parts[1]) - 1, Number(parts[2]))
  }

  function selectedDateLabel() {
    var date = root.selectedDate
    return root.labelLocale.dayName(date.getDay(), Locale.LongFormat)
      + ", " + Model.ordinalDay(date.getDate())
      + " " + root.labelLocale.monthName(date.getMonth(), Locale.LongFormat)
  }

  function syncSelectionFromService() {
    if (!root.actualService || root.actualService.selectedDate === undefined) return
    root.selectedDateKey = Model.normalizedDateKey(root.actualService.selectedDate, root.selectedDateKey)
  }

  function selectDateKey(key) {
    var normalized = Model.normalizedDateKey(key, root.selectedDateKey)
    if (normalized === "") return
    root.selectedDateKey = normalized
    if (root.actualService && typeof root.actualService.selectDate === "function")
      root.actualService.selectDate(normalized)
  }

  function selectCell(cell) {
    if (!cell) return
    if (cell.year !== root.viewYear || cell.month !== root.viewMonth) {
      root.viewYear = cell.year
      root.viewMonth = cell.month
    }
    root.selectDateKey(cell.key)
  }

  function requestRange() {
    if (!root.actualService || !root.weeks || root.weeks.length === 0) return
    var first = root.weeks[0].days[0].key
    var lastCell = root.weeks[root.weeks.length - 1].days[6]
    var exclusiveEnd = Model.keyForDate(new Date(lastCell.year, lastCell.month, lastCell.day + 1))
    var token = first + "/" + exclusiveEnd
    if (token === root.requestedRange) return
    root.requestedRange = token
    if (typeof root.actualService.loadRange === "function")
      root.actualService.loadRange(first, exclusiveEnd)
    else if (typeof root.actualService.setAgendaRange === "function")
      root.actualService.setAgendaRange(first, exclusiveEnd)
  }

  function refreshCalendars() {
    if (!root.actualService || typeof root.actualService.refresh !== "function") return
    root.actualService.refresh()
  }

  function addAccount() {
    if (!root.addingAccount && !root.cancellingAccount && !root.oauthFinishing
        && root.actualService && typeof root.actualService.addAccount === "function")
      root.actualService.addAccount()
  }

  function cancelAddAccount() {
    if (root.oauthCancelable && !root.cancellingAccount && root.actualService
        && typeof root.actualService.cancelAddAccount === "function")
      root.actualService.cancelAddAccount()
  }

  function removeSelectedAccount() {
    if (!root.actualService || root.accountFilter === "all" || typeof root.actualService.removeAccount !== "function") return
    var id = root.accountFilter
    if (typeof root.actualService.setAccountFilter === "function") root.actualService.setAccountFilter("all")
    root.actualService.removeAccount(id)
  }

  function setAccountFilter(id) {
    if (root.actualService && typeof root.actualService.setAccountFilter === "function")
      root.actualService.setAccountFilter(String(id || "all"))
  }

  function openAgendaItem(item) {
    if (!item || !root.actualService || typeof root.actualService.openItem !== "function") return
    var id = String(item.id || "")
    if (id === "" || root.openingItemId !== "") return
    root.actualService.openItem(id)
  }

  function serviceStatusText() {
    if (!root.actualService) return "Calendar service unavailable"
    var accountCount = root.serviceAccounts.length
    var prefix = accountCount + " account" + (accountCount === 1 ? "" : "s")
    if (!root.backendConnected || root.serviceStatus === "connecting") return prefix + " · connecting…"
    if (root.cancellingAccount) return "Cancelling Google sign-in…"
    if (root.oauthFinishing) return "Finishing Google sign-in…"
    if (root.addingAccount && root.oauthCancelable) return "Waiting for Google sign-in…"
    if (root.addingAccount) return "Opening Google sign-in…"
    if (root.openingItemId !== "") return "Opening event…"
    if (root.serviceStatus === "not_configured") return "Google OAuth setup needed"
    if (root.serviceStatus === "empty") return "No accounts connected"
    if (root.serviceBusy) return prefix + " · syncing…"
    if (root.serviceStatus === "offline") return prefix + " · cached offline"
    if (root.serviceError !== "" || root.serviceStatus === "error") return prefix + " · sync issue"
    return prefix + " · synced"
  }

  function emptyAgendaText() {
    if (!root.actualService) return "Calendar service unavailable"
    if (!root.backendConnected || root.serviceStatus === "connecting") return "Connecting to calendar…"
    if (root.serviceStatus === "not_configured") return "Google setup required"
    if (root.serviceStatus === "empty") return "Connect a Google account"
    return "Nothing scheduled"
  }

  function agendaTime(item) {
    if (!item) return ""
    if (String(item.type || "event").toLowerCase() === "task") return "Due today"
    if (item.allDay === true) return "All day"
    var start = new Date(item.start)
    return isNaN(start.getTime()) ? "" : Qt.formatTime(start, "HH:mm")
  }

  function agendaSource(item) {
    if (!item) return ""
    if (item.sourceLabels && item.sourceLabels.length > 0) return item.sourceLabels.join(", ")
    return String(item.accountLabel || item.calendarLabel || "")
  }

  function agendaMeta(item) {
    if (!item) return ""
    var parts = []
    var source = root.agendaSource(item)
    if (source !== "") parts.push(source)
    var calendar = String(item.calendarLabel || "")
    if (calendar !== "" && calendar !== source) parts.push(calendar)
    var location = String(item.location || "")
    if (location !== "") parts.push(location)
    var reminders = Model.reminderSummary(item.reminders)
    if (reminders !== "") parts.push("󰂚 " + reminders)
    if (String(item.type || "event").toLowerCase() === "task") parts.push("Google Tasks · date only")
    return parts.join(" · ")
  }

  function open() {
    root.controller.show()
    root.refreshView()
    // Set after showing, not before: showing hands the popout coordinator
    // over, which closes whichever panel was open, and that close clears the
    // shared flag. Deferring means the panel taking over always wins, while
    // a handoff to a panel that does not manage the flag still leaves it
    // cleared rather than stuck on.
    Qt.callLater(function() {
      if (root.opened) setCenterHoverRevealSuppressed(true)
    })
  }

  function close() {
    setCenterHoverRevealSuppressed(false)
    // Dismissing the panel mid-edit would otherwise leave the inputs up,
    // waiting behind a closed popup for the next time it opens.
    if (root.editingLife) root.cancelEditingLife()
    root.controller.hide()
  }

  function toggle() {
    if (root.opened) root.close()
    else root.open()
  }

  function switchPanel(direction) {
    if (root.bar && typeof root.bar.switchPanelFrom === "function")
      return root.bar.switchPanelFrom(root.barIdentity, direction)
    return false
  }

  // Summoning by hotkey moves no pointer, so a hover the bar was still
  // holding must not keep the center indicators revealed behind the panel.
  function setCenterHoverRevealSuppressed(value) {
    if (root.bar && "centerHoverRevealSuppressed" in root.bar)
      root.bar.centerHoverRevealSuppressed = value
  }

  function refreshView() {
    root.today = new Date()
    root.goToToday()
  }

  function refresh() {
    root.refreshView()
  }

  function refreshFromGoogle() {
    root.refreshView()
    root.refreshCalendars()
  }

  function goToToday() {
    root.viewYear = today.getFullYear()
    root.viewMonth = today.getMonth()
    root.selectDateKey(root.todayKey)
  }

  function moveMonth(delta) {
    var next = Model.stepMonth(viewYear, viewMonth, delta)
    var selected = root.dateFromKey(root.selectedDateKey)
    var targetDay = Math.min(selected.getDate(), new Date(next.year, next.month + 1, 0).getDate())
    root.viewYear = next.year
    root.viewMonth = next.month
    root.selectDateKey(Model.dateKey(next.year, next.month, targetDay))
  }

  function moveYear(delta) {
    moveMonth(delta * 12)
  }

  // Applied locally first so the panel redraws on the click itself; the
  // shell.json write comes back through the bar as the same value. With no
  // writable entry (the widget is not in the layout) it stays a session-only
  // preference rather than doing nothing. The host widget builds its own
  // entry when the label format is cycled, so it has to be kept in step or
  // it would write this key straight back out from a stale copy.
  function persistSettings(values) {
    var entry = { id: root.moduleName }
    for (var existing in root.settings) if (existing !== "id") entry[existing] = root.settings[existing]
    for (var key in values) entry[key] = values[key]

    root.settings = entry
    if (root.hostWidget && "settings" in root.hostWidget) root.hostWidget.settings = entry
    if (root.bar && root.bar.shell && typeof root.bar.shell.updateEntryInline === "function")
      root.bar.shell.updateEntryInline(root.moduleName, entry)
  }

  function setWeekStart(day) {
    var next = Model.normalizedWeekStart(day, root.weekStart)
    if (next === root.weekStart) return
    persistSettings({ weekStartDay: Model.weekStartSettingName(next) })
  }

  function startEditingLife() {
    root.editingLife = true
    Qt.callLater(function() {
      bornField.text = root.birthYear > 0 ? String(root.birthYear) : ""
      expectancyField.text = String(root.lifeExpectancy)
      bornField.selectAll()
      bornField.forceActiveFocus()
    })
  }

  function cancelEditingLife() {
    root.editingLife = false
    Qt.callLater(function() { if (keyCatcher) keyCatcher.forceActiveFocus() })
  }

  // Shared by both fields: Tab hops to the other one, Enter commits the pair,
  // Escape drops the lot.
  function handleLifeKey(event, other) {
    if (event.key === Qt.Key_Escape) {
      root.cancelEditingLife()
      event.accepted = true
    } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
      root.commitLife()
      event.accepted = true
    } else if (event.key === Qt.Key_Tab || event.key === Qt.Key_Backtab) {
      other.selectAll()
      other.forceActiveFocus()
      event.accepted = true
    }
  }

  // Double-tapping the life bar puts it away again. The expectancy stays in
  // the config so setting a birth year again brings your own number back
  // rather than the default.
  function clearLife() {
    if (root.birthYear <= 0) return
    persistSettings({ birthYear: 0 })
  }

  function commitLife() {
    var born = Model.parseBirthYear(bornField.text, today.getFullYear())
    var span = Model.parseLifeExpectancy(expectancyField.text)
    if (born !== root.birthYear || span !== root.lifeExpectancy)
      persistSettings({ birthYear: born, lifeExpectancy: span })
    cancelEditingLife()
  }

  function toggleWeekStart() {
    setWeekStart(Model.toggledWeekStart(root.weekStart))
  }

  // English short day names, matching the rest of the interface.
  function weekdayLabel(weekday) {
    return String(labelLocale.dayName(weekday, Locale.ShortFormat)).toUpperCase()
  }

  SystemClock {
    id: clock
    precision: SystemClock.Minutes
    onDateChanged: {
      if (Model.keyForDate(clock.date) === String(root.todayKey)) return
      var followToday = root.viewingCurrentMonth
      root.today = clock.date
      if (followToday) root.goToToday()
    }
  }

  Connections {
    target: root.actualService
    ignoreUnknownSignals: true

    function onSelectedDateChanged() { root.syncSelectionFromService() }
  }

  onActualServiceChanged: {
    root.requestedRange = ""
    root.syncSelectionFromService()
    Qt.callLater(root.requestRange)
  }
  onViewYearChanged: {
    root.requestedRange = ""
    Qt.callLater(root.requestRange)
  }
  onViewMonthChanged: {
    root.requestedRange = ""
    Qt.callLater(root.requestRange)
  }
  Component.onCompleted: Qt.callLater(root.requestRange)

  KeyboardPanel {
    id: panel
    anchorItem: root.anchorItem
    owner: root.barIdentity
    bar: root.bar
    open: root.opened
    centerOnBar: true
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(840))
    contentHeight: panel.fittedContentHeight(calendarColumn.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      blocked: root.editingLife
      onMoveRequested: function(dx, dy) {
        if (dx !== 0) root.moveMonth(dx)
        if (dy !== 0) root.moveYear(dy)
      }
      onActivateRequested: root.goToToday()
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }
      onTextKey: function(t) {
        if (t === "[") root.moveMonth(-1)
        else if (t === "]") root.moveMonth(1)
        else if (t === "{") root.moveYear(-1)
        else if (t === "}") root.moveYear(1)
        else if (t === "t" || t === "T") root.goToToday()
        else if (t === "w" || t === "W") root.toggleWeekStart()
      }

      Flickable {
        id: calendarScroll
        anchors.fill: parent
        contentWidth: calendarColumn.width
        contentHeight: calendarColumn.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        interactive: contentHeight > height || contentWidth > width

        Column {
          id: calendarColumn
          // Never narrower than the grid. The popup width is capped to what
          // the screen allows, and a fixed seven-column grid would otherwise
          // lose its last days off the edge instead of scrolling.
          width: Math.max(calendarScroll.width, gridColumn.width)
          spacing: Style.space(8)

          Item {
            width: parent.width
            height: Math.max(calendarTitle.implicitHeight + calendarStatus.implicitHeight + Style.space(4), toolbarActions.implicitHeight)

            Column {
              anchors.left: parent.left
              anchors.verticalCenter: parent.verticalCenter
              width: Math.max(0, parent.width - toolbarActions.width - Style.space(20))
              spacing: Style.space(3)

              Text {
                id: calendarTitle
                text: "Calendar"
                color: root.contentForeground
                font.family: root.contentFontFamily
                font.pixelSize: Style.font.title
                font.bold: true
              }

              Text {
                id: calendarStatus
                width: parent.width
                text: root.serviceStatusText()
                textFormat: Text.PlainText
                elide: Text.ElideRight
                color: Qt.darker(root.contentForeground, 1.45)
                font.family: root.contentFontFamily
                font.pixelSize: Style.font.caption
              }
            }

            Row {
              id: toolbarActions
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              spacing: Style.space(4)

              PanelActionButton {
                iconText: "󰑐"
                tooltipText: "Refresh calendars"
                foreground: root.contentForeground
                fontFamily: root.contentFontFamily
                enabled: root.backendConnected && root.serviceAccounts.length > 0 && !root.serviceBusy
                onClicked: root.refreshFromGoogle()
              }

              PanelActionButton {
                iconText: root.oauthCancelable ? "×" : "+"
                tooltipText: root.cancellingAccount
                  ? "Cancelling Google sign-in…"
                  : (root.oauthFinishing
                    ? "Finishing Google sign-in…"
                    : (root.oauthCancelable
                      ? "Cancel Google sign-in"
                      : (root.addingAccount ? "Opening Google sign-in…" : "Add Google account")))
                foreground: root.contentForeground
                fontFamily: root.contentFontFamily
                enabled: root.backendConnected && !root.cancellingAccount && !root.oauthFinishing
                  && (!root.addingAccount || root.oauthCancelable)
                onClicked: {
                  if (root.oauthCancelable) root.cancelAddAccount()
                  else root.addAccount()
                }
              }

              PanelActionButton {
                visible: root.accountFilter !== "all"
                iconText: "󰆴"
                tooltipText: "Remove selected account"
                foreground: root.contentForeground
                hoverColor: root.bar ? root.bar.urgent : Color.urgent
                fontFamily: root.contentFontFamily
                enabled: root.backendConnected
                onClicked: root.removeSelectedAccount()
              }
            }
          }

          BorderSurface {
            visible: root.serviceError !== ""
            width: parent.width
            implicitHeight: syncError.implicitHeight + Style.space(18)
            color: Qt.rgba((root.bar ? root.bar.urgent : Color.urgent).r, (root.bar ? root.bar.urgent : Color.urgent).g, (root.bar ? root.bar.urgent : Color.urgent).b, 0.10)
            borderSpec: Border.flat(Qt.rgba((root.bar ? root.bar.urgent : Color.urgent).r, (root.bar ? root.bar.urgent : Color.urgent).g, (root.bar ? root.bar.urgent : Color.urgent).b, 0.35), 1)
            radius: Style.cornerRadius

            Text {
              id: syncError
              anchors.left: parent.left
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              anchors.margins: Style.space(9)
              text: root.serviceError
              textFormat: Text.PlainText
              color: root.contentForeground
              font.family: root.contentFontFamily
              font.pixelSize: Style.font.caption
              wrapMode: Text.WordWrap
            }
          }

          Flow {
            id: accountFilters
            width: parent.width
            spacing: Style.space(6)

            Button {
              text: "All accounts"
              selected: root.accountFilter === "all"
              bordered: true
              foreground: root.contentForeground
              fontFamily: root.contentFontFamily
              fontSize: Style.font.bodySmall
              horizontalPadding: Style.space(10)
              verticalPadding: Style.space(5)
              onClicked: root.setAccountFilter("all")
            }

            Repeater {
              model: root.serviceAccounts

              BorderSurface {
                required property var modelData
                readonly property bool chosen: root.accountFilter === String(modelData.id || "")
                readonly property color accountColor: modelData.color
                  ? modelData.color
                  : (modelData.calendars && modelData.calendars.length > 0 && modelData.calendars[0].color
                    ? modelData.calendars[0].color
                    : Color.accent)

                implicitWidth: accountChipRow.implicitWidth + Style.space(20)
                implicitHeight: accountChipRow.implicitHeight + Style.space(10)
                radius: height / 2
                color: chosen
                  ? Style.selectedFillFor(root.contentForeground, accountColor)
                  : (accountChipMouse.containsMouse ? Style.hoverFillFor(root.contentForeground, accountColor) : "transparent")
                borderSpec: Border.flat(chosen
                  ? Style.selectedStateColor(root.contentForeground, accountColor)
                  : Qt.rgba(root.contentForeground.r, root.contentForeground.g, root.contentForeground.b, 0.20), 1)

                Row {
                  id: accountChipRow
                  anchors.centerIn: parent
                  spacing: Style.space(6)

                  Rectangle {
                    anchors.verticalCenter: parent.verticalCenter
                    width: Style.space(7)
                    height: width
                    radius: width / 2
                    color: parent.parent.accountColor
                  }

                  Text {
                    anchors.verticalCenter: parent.verticalCenter
                    text: String(modelData.label || modelData.email || "Account")
                    textFormat: Text.PlainText
                    color: accountChipRow.parent.chosen
                      ? Style.selectedStateColor(root.contentForeground, accountChipRow.parent.accountColor)
                      : root.contentForeground
                    font.family: root.contentFontFamily
                    font.pixelSize: Style.font.bodySmall
                    font.bold: accountChipRow.parent.chosen
                  }
                }

                MouseArea {
                  id: accountChipMouse
                  anchors.fill: parent
                  hoverEnabled: true
                  cursorShape: Qt.PointingHandCursor
                  onClicked: root.setAccountFilter(String(modelData.id || "all"))
                }

                PanelToolTip {
                  visible: accountChipMouse.containsMouse
                  text: modelData.connected === false
                    ? String(modelData.error || "Account needs reconnecting")
                    : String(modelData.email || modelData.label || "")
                  fontFamily: root.contentFontFamily
                }
              }
            }
          }

          PanelSeparator {
            foreground: root.contentForeground
          }

          Grid {
            id: mainGrid
            width: parent.width
            columns: root.wideLayout ? 2 : 1
            columnSpacing: Style.space(20)
            rowSpacing: Style.space(20)

            Column {
              id: monthPane
              width: root.wideLayout
                ? Math.max(gridColumn.width, Math.floor((mainGrid.width - mainGrid.columnSpacing) * 0.54))
                : mainGrid.width
              spacing: Style.space(8)

              Item {
                width: parent.width
                height: compactMonthLabel.implicitHeight + Style.space(8)

                Text {
                  id: compactMonthLabel
                  anchors.left: parent.left
                  anchors.verticalCenter: parent.verticalCenter
                  text: Qt.formatDate(root.viewDate, "MMMM yyyy")
                  textFormat: Text.PlainText
                  color: root.contentForeground
                  font.family: root.contentFontFamily
                  font.pixelSize: Style.font.subtitle
                  font.bold: true
                }

                Row {
                  anchors.right: parent.right
                  anchors.verticalCenter: parent.verticalCenter
                  spacing: Style.space(3)

                  PanelActionButton {
                    iconText: "󰅁"
                    tooltipText: "Previous month"
                    foreground: root.contentForeground
                    fontFamily: root.contentFontFamily
                    onClicked: root.moveMonth(-1)
                  }

                  PanelActionButton {
                    iconText: "󰅂"
                    tooltipText: "Next month"
                    foreground: root.contentForeground
                    fontFamily: root.contentFontFamily
                    onClicked: root.moveMonth(1)
                  }
                }
              }

          // ---- Hero: today, centered. Once the view has stepped back
          //      it is also the way home — clicking the date you are
          //      looking for beats hunting for a reset button.
          Item {
            visible: false
            width: parent.width
            height: 0

            Row {
              id: heroRow
              anchors.horizontalCenter: parent.horizontalCenter
              spacing: Style.space(22)

              Text {
                // Baseline-aligned, not center-aligned: "July 26" carries a
                // descender, so centering the two boxes leaves the icon
                // sitting visibly low against the digits.
                anchors.baseline: heroDate.baseline
                text: "󰃭"
                color: heroMouse.containsMouse
                  ? Style.hoverStateColor(root.contentForeground, Color.accent)
                  : root.contentForeground
                font.family: root.contentFontFamily
                // Decorative, and deliberately outside the Style.font.*
                // scale. Sized so the glyph reads at the cap height of the
                // date beside it rather than towering over it.
                font.pixelSize: 48
              }

              Text {
                id: heroDate
                textFormat: Text.PlainText
                anchors.verticalCenter: parent.verticalCenter
                text: Qt.formatDate(root.today, "MMMM d")
                color: heroMouse.containsMouse
                  ? Style.hoverStateColor(root.contentForeground, Color.accent)
                  : root.contentForeground
                font.family: root.contentFontFamily
                font.pixelSize: 52
                font.bold: true
              }
            }

            MouseArea {
              id: heroMouse
              x: heroRow.x
              y: heroRow.y
              width: heroRow.width
              height: heroRow.height
              enabled: !root.viewingCurrentMonth
              hoverEnabled: enabled
              cursorShape: Qt.PointingHandCursor
              onClicked: root.goToToday()

              PanelToolTip {
                visible: heroMouse.containsMouse
                text: "Back to today"
                fontFamily: root.contentFontFamily
              }
            }
          }

          // ---- Year progress, doubling as the rule under the hero:
          //      a plain hairline said nothing, and whole days done
          //      over days in the year says the same thing louder.
          Item {
            width: parent.width
            height: yearBlock.y + yearBlock.height

            Item {
              id: yearBlock
              y: Style.space(6)
              anchors.horizontalCenter: parent.horizontalCenter
              width: gridColumn.width
              height: Math.max(yearLabel.implicitHeight, Style.space(10))

              TapHandler {
                enabled: !root.editingLife
                onDoubleTapped: root.startEditingLife()
              }

              Row {
                visible: root.editingLife
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.verticalCenter: parent.verticalCenter
                spacing: Style.space(10)

                Text {
                  anchors.verticalCenter: parent.verticalCenter
                  text: "BORN"
                  color: Qt.darker(root.contentForeground, 1.5)
                  font.family: root.contentFontFamily
                  font.pixelSize: Style.font.bodySmall
                  font.letterSpacing: 1
                }

                TextField {
                  id: bornField
                  width: Style.space(70)
                  anchors.verticalCenter: parent.verticalCenter
                  placeholderText: "year"
                  foreground: root.contentForeground
                  font.family: root.contentFontFamily
                  inputMethodHints: Qt.ImhDigitsOnly

                  Keys.onPressed: function(event) { root.handleLifeKey(event, expectancyField) }
                }

                Text {
                  anchors.verticalCenter: parent.verticalCenter
                  anchors.verticalCenterOffset: 0
                  leftPadding: Style.space(6)
                  text: "LIVE TO"
                  color: Qt.darker(root.contentForeground, 1.5)
                  font.family: root.contentFontFamily
                  font.pixelSize: Style.font.bodySmall
                  font.letterSpacing: 1
                }

                TextField {
                  id: expectancyField
                  width: Style.space(60)
                  anchors.verticalCenter: parent.verticalCenter
                  placeholderText: "90"
                  foreground: root.contentForeground
                  font.family: root.contentFontFamily
                  inputMethodHints: Qt.ImhDigitsOnly

                  Keys.onPressed: function(event) { root.handleLifeKey(event, bornField) }
                }
              }

              Text {
                id: yearLabel
                textFormat: Text.PlainText
                visible: !root.editingLife
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                text: root.today.getFullYear()
                color: Qt.darker(root.contentForeground, 1.5)
                font.family: root.contentFontFamily
                font.pixelSize: Style.font.bodySmall
                font.letterSpacing: 1
              }

              Text {
                id: yearPercent
                textFormat: Text.PlainText
                visible: !root.editingLife
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                text: root.yearDonePercent + "%"
                color: root.contentForeground
                font.family: root.contentFontFamily
                font.pixelSize: Style.font.bodySmall
              }

              Rectangle {
                id: yearTrack
                visible: !root.editingLife
                anchors.left: yearLabel.right
                anchors.right: yearPercent.left
                anchors.leftMargin: Style.space(12)
                anchors.rightMargin: Style.space(12)
                anchors.verticalCenter: parent.verticalCenter
                height: Style.space(6)
                radius: Style.cornerRadius > 0 ? height / 2 : 0
                color: Qt.rgba(root.contentForeground.r, root.contentForeground.g, root.contentForeground.b, 0.12)

                Rectangle {
                  width: Math.round(parent.width * root.yearDone)
                  height: parent.height
                  radius: parent.radius
                  color: Style.selectedStateColor(root.contentForeground, Color.accent)

                  Behavior on width { NumberAnimation { duration: 160; easing.type: Easing.OutCubic } }
                }
              }
            }
          }

          // ---- Memento mori. Only here once someone has gone looking and
          //      given an age; the same rail as the year above it, measured
          //      against a nominal lifetime.
          Item {
            visible: root.birthYear > 0
            width: parent.width
            height: visible ? lifeBlock.height : 0

            Item {
              id: lifeBlock
              anchors.horizontalCenter: parent.horizontalCenter
              width: gridColumn.width
              height: Math.max(lifeLabel.implicitHeight, Style.space(10))

              Text {
                id: lifeLabel
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                text: "LIFE"
                color: Qt.darker(root.contentForeground, 1.5)
                font.family: root.contentFontFamily
                font.pixelSize: Style.font.bodySmall
                font.letterSpacing: 1
              }

              Text {
                id: lifePercent
                textFormat: Text.PlainText
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                text: root.lifeDonePercent + "%"
                color: root.contentForeground
                font.family: root.contentFontFamily
                font.pixelSize: Style.font.bodySmall
              }

              Rectangle {
                anchors.left: lifeLabel.right
                anchors.right: lifePercent.left
                anchors.leftMargin: Style.space(12)
                anchors.rightMargin: Style.space(12)
                anchors.verticalCenter: parent.verticalCenter
                height: Style.space(6)
                radius: Style.cornerRadius > 0 ? height / 2 : 0
                color: Qt.rgba(root.contentForeground.r, root.contentForeground.g, root.contentForeground.b, 0.12)

                Rectangle {
                  width: Math.round(parent.width * root.lifeDone)
                  height: parent.height
                  radius: parent.radius
                  color: Style.selectedStateColor(root.contentForeground, Color.accent)

                  Behavior on width { NumberAnimation { duration: 160; easing.type: Easing.OutCubic } }
                }
              }

              TapHandler {
                onDoubleTapped: root.clearLife()
              }

              MouseArea {
                id: lifeMouse
                anchors.fill: parent
                hoverEnabled: true
                acceptedButtons: Qt.NoButton

                PanelToolTip {
                  visible: lifeMouse.containsMouse
                  text: "Memento Mori"
                  fontFamily: root.contentFontFamily
                }
              }
            }
          }

          // ---- Month grid: week numbers down a gutter on the left, then
          //      the seven day columns. Always six rows, so the popup is
          //      exactly as tall in February as it is in August.
          Item {
            width: parent.width
            height: gridColumn.y + gridColumn.height

            WheelHandler {
              onWheel: function(event) {
                // Horizontal wheels and touchpad side-scrolls report y === 0;
                // without this they would every one read as "next month".
                if (event.angleDelta.y === 0) return
                root.moveMonth(event.angleDelta.y > 0 ? -1 : 1)
              }
            }

            Column {
              id: gridColumn
              // The meter above is a solid rule; the grid needs room to
              // read as its own block rather than hanging off it.
              y: Style.space(18)
              anchors.horizontalCenter: parent.horizontalCenter
              spacing: Style.space(3)

              Row {
                id: headerRow
                spacing: root.cellSpacing

                // The week-number heading doubles as the week-start toggle.
                // It is the one control in the panel whose meaning is not
                // self-evident, so it carries a tooltip naming the day the
                // click will switch to.
                Rectangle {
                  width: root.weekColumnWidth
                  height: Style.space(16)
                  radius: Style.cornerRadius
                  color: weekStartMouse.containsMouse
                    ? Style.hoverFillFor(root.contentForeground, Color.accent)
                    : "transparent"

                  Text {
                    anchors.centerIn: parent
                    text: "W"
                    color: weekStartMouse.containsMouse
                      ? Style.hoverStateColor(root.contentForeground, Color.accent)
                      : Qt.darker(root.contentForeground, 1.9)
                    font.family: root.contentFontFamily
                    font.pixelSize: Style.font.caption
                    font.letterSpacing: 1
                    font.bold: true
                  }

                  MouseArea {
                    id: weekStartMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: root.toggleWeekStart()
                  }

                  PanelToolTip {
                    visible: weekStartMouse.containsMouse
                    text: "Start weeks on " + root.nextWeekStartLabel
                    fontFamily: root.contentFontFamily
                  }
                }

                Item {
                  width: root.gutterWidth
                  height: Style.space(16)
                }

                Repeater {
                  model: root.weekdays

                  Text {
                    textFormat: Text.PlainText
                    required property var modelData
                    width: root.cellWidth
                    height: Style.space(16)
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                    text: root.weekdayLabel(modelData)
                    color: Qt.darker(root.contentForeground, 1.5)
                    font.family: root.contentFontFamily
                    font.pixelSize: Style.font.caption
                    font.letterSpacing: 1
                    font.bold: true
                  }
                }
              }

              Repeater {
                model: root.weeks

                Row {
                  required property var modelData
                  spacing: root.cellSpacing

                  Text {
                    textFormat: Text.PlainText
                    width: root.weekColumnWidth
                    height: root.cellHeight
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                    text: modelData.week
                    color: Qt.darker(root.contentForeground, 1.9)
                    font.family: root.contentFontFamily
                    font.pixelSize: Style.font.caption
                  }

                  Item {
                    width: root.gutterWidth
                    height: root.cellHeight
                  }

                  Repeater {
                    model: modelData.days

                    Rectangle {
                      id: dayCell
                      required property var modelData
                      readonly property bool selectedCell: modelData.key === root.selectedDateKey
                      readonly property var dotColors: Model.dayDotColors(root.serviceItems, modelData.key, root.accountFilter, 3)

                      width: root.cellWidth
                      height: root.cellHeight
                      radius: Style.cornerRadius
                      color: selectedCell
                        ? Style.selectedFillFor(root.contentForeground, Color.accent)
                        : (dayCellMouse.containsMouse ? Style.hoverFillFor(root.contentForeground, Color.accent) : "transparent")
                      border.width: modelData.today && !selectedCell ? Style.spacing.hairline : 0
                      border.color: Style.normalBorderFor(root.contentForeground, Color.accent)

                      Column {
                        anchors.centerIn: parent
                        spacing: dotRow.visible ? Style.space(2) : 0

                        Text {
                          anchors.horizontalCenter: parent.horizontalCenter
                          textFormat: Text.PlainText
                          text: modelData.day
                          color: dayCell.selectedCell
                            ? Style.selectedStateColor(root.contentForeground, Color.accent)
                            : (modelData.inMonth
                              ? (modelData.weekend ? Qt.darker(root.contentForeground, 1.45) : root.contentForeground)
                              : Qt.darker(root.contentForeground, 2.2))
                          font.family: root.contentFontFamily
                          font.pixelSize: Style.font.body
                          font.bold: modelData.today || dayCell.selectedCell
                        }

                        Row {
                          id: dotRow
                          visible: dayCell.dotColors.length > 0
                          anchors.horizontalCenter: parent.horizontalCenter
                          spacing: Style.space(2)

                          Repeater {
                            model: dayCell.dotColors

                            Rectangle {
                              required property var modelData
                              width: Style.space(5)
                              height: width
                              radius: width / 2
                              color: dayCell.selectedCell
                                ? Style.selectedStateColor(root.contentForeground, Color.accent)
                                : modelData
                            }
                          }
                        }
                      }

                      MouseArea {
                        id: dayCellMouse
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.selectCell(dayCell.modelData)
                      }

                      PanelToolTip {
                        visible: dayCellMouse.containsMouse
                        text: Model.agendaItemsForDay(root.serviceItems, modelData.key, root.accountFilter).length
                          + " item(s)"
                        fontFamily: root.contentFontFamily
                      }
                    }
                  }
                }
              }
            }

            // Hairline down the week-number gutter, drawn only beside the
            // day rows so it does not cut through the header band.
            Rectangle {
              x: gridColumn.x + root.weekColumnWidth + root.cellSpacing + Math.round((root.gutterWidth - width) / 2)
              y: gridColumn.y + headerRow.height + gridColumn.spacing
              width: Style.spacing.hairline
              height: gridColumn.height - headerRow.height - gridColumn.spacing
              color: root.contentForeground
              opacity: 0.1
            }
          }

          // ---- Month stepping, spanning the grid it drives. The chevrons
          //      sit on the grid's outer bounds, the same edges the year
          //      rail above uses, so the row reads as the panel's other
          //      full-width rail instead of a cluster floating in space.
          //      The label is centered and fixed-width, so it holds still
          //      from "MAY" to "SEPTEMBER".
          Item {
            visible: false
            width: parent.width
            height: 0

            Item {
              id: monthNav
              anchors.horizontalCenter: parent.horizontalCenter
              width: gridColumn.width
              height: monthLabel.implicitHeight + Style.space(10)

              Text {
                id: monthLabel
                textFormat: Text.PlainText
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.verticalCenter: parent.verticalCenter
                // Fixed width so the chevrons hold still between a
                // "MAY 2026" and a "SEPTEMBER 2026".
                width: Style.space(130)
                horizontalAlignment: Text.AlignHCenter
                text: Qt.formatDate(root.viewDate, "MMMM yyyy").toUpperCase()
                color: Qt.darker(root.contentForeground, 1.4)
                font.family: root.contentFontFamily
                font.pixelSize: Style.font.body
                font.letterSpacing: 1
              }

              PanelActionButton {
                // Pulled out by the button's own padding so the glyph, not
                // its hit box, lines up with the "2026" on the year rail.
                anchors.left: parent.left
                anchors.leftMargin: -Style.space(8)
                anchors.verticalCenter: parent.verticalCenter
                iconText: "󰅁"
                tooltipText: "Previous month"
                foreground: root.contentForeground
                fontFamily: root.contentFontFamily
                onClicked: root.moveMonth(-1)
              }

              PanelActionButton {
                anchors.right: parent.right
                anchors.rightMargin: -Style.space(8)
                anchors.verticalCenter: parent.verticalCenter
                iconText: "󰅂"
                tooltipText: "Next month"
                foreground: root.contentForeground
                fontFamily: root.contentFontFamily
                onClicked: root.moveMonth(1)
              }
            }
          }

            }

            BorderSurface {
              id: agendaPane
              width: root.wideLayout
                ? Math.max(Style.space(300), mainGrid.width - monthPane.width - mainGrid.columnSpacing)
                : mainGrid.width
              implicitHeight: agendaColumn.implicitHeight + Style.space(24)
              radius: Style.cornerRadius
              color: Qt.rgba(root.contentForeground.r, root.contentForeground.g, root.contentForeground.b, 0.035)
              borderSpec: Border.flat(Qt.rgba(root.contentForeground.r, root.contentForeground.g, root.contentForeground.b, 0.12), 1)

              Column {
                id: agendaColumn
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.margins: Style.space(12)
                spacing: Style.space(8)

                Item {
                  width: parent.width
                  height: Math.max(agendaDateLabel.implicitHeight, agendaCount.implicitHeight)

                  Text {
                    id: agendaDateLabel
                    anchors.left: parent.left
                    anchors.right: agendaCount.left
                    anchors.rightMargin: Style.space(10)
                    anchors.verticalCenter: parent.verticalCenter
                    text: root.selectedDateLabel()
                    textFormat: Text.PlainText
                    elide: Text.ElideRight
                    color: root.contentForeground
                    font.family: root.contentFontFamily
                    font.pixelSize: Style.font.subtitle
                    font.bold: true
                  }

                  Text {
                    id: agendaCount
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    text: root.agendaItems.length + " item" + (root.agendaItems.length === 1 ? "" : "s")
                    textFormat: Text.PlainText
                    color: Qt.darker(root.contentForeground, 1.5)
                    font.family: root.contentFontFamily
                    font.pixelSize: Style.font.caption
                  }
                }

                PanelSeparator {
                  foreground: root.contentForeground
                }

                Text {
                  visible: root.agendaItems.length === 0
                  width: parent.width
                  topPadding: Style.space(24)
                  bottomPadding: Style.space(24)
                  text: root.emptyAgendaText()
                  textFormat: Text.PlainText
                  color: Qt.darker(root.contentForeground, 1.5)
                  font.family: root.contentFontFamily
                  font.pixelSize: Style.font.body
                  horizontalAlignment: Text.AlignHCenter
                }

                Repeater {
                  model: root.agendaItems

                  BorderSurface {
                    required property var modelData
                    readonly property color sourceColor: modelData.sourceColors && modelData.sourceColors.length > 0
                      ? modelData.sourceColors[0]
                      : (modelData.color || Color.accent)
                    readonly property bool openingItem: root.openingItemId === String(modelData.id || "")

                    width: agendaColumn.width
                    implicitHeight: Math.max(Style.space(62), agendaItemContent.implicitHeight + Style.space(18))
                    radius: Style.cornerRadius
                    color: openingItem
                      ? Style.selectedFillFor(root.contentForeground, sourceColor)
                      : (agendaItemMouse.containsMouse
                        ? Style.hoverFillFor(root.contentForeground, sourceColor)
                        : Qt.rgba(root.contentForeground.r, root.contentForeground.g, root.contentForeground.b, 0.025))
                    borderSpec: Border.flat(openingItem
                      ? Style.selectedStateColor(root.contentForeground, sourceColor)
                      : (agendaItemMouse.containsMouse
                        ? Style.hoverStateColor(root.contentForeground, sourceColor)
                        : Qt.rgba(root.contentForeground.r, root.contentForeground.g, root.contentForeground.b, 0.12)), 1)

                    Row {
                      id: agendaItemContent
                      anchors.left: parent.left
                      anchors.right: parent.right
                      anchors.verticalCenter: parent.verticalCenter
                      anchors.leftMargin: Style.space(10)
                      anchors.rightMargin: Style.space(10)
                      spacing: Style.space(10)

                      Text {
                        width: Style.space(62)
                        anchors.top: parent.top
                        topPadding: Style.space(2)
                        text: root.agendaTime(modelData)
                        textFormat: Text.PlainText
                        color: Qt.darker(root.contentForeground, 1.35)
                        font.family: root.contentFontFamily
                        font.pixelSize: Style.font.caption
                      }

                      Column {
                        width: Math.max(0, agendaItemContent.width - Style.space(72))
                        spacing: Style.space(5)

                        Row {
                          width: parent.width
                          spacing: Style.space(6)

                          Rectangle {
                            anchors.verticalCenter: parent.verticalCenter
                            width: Style.space(7)
                            height: width
                            radius: width / 2
                            color: agendaItemContent.parent.sourceColor
                          }

                          Text {
                            width: Math.max(0, parent.width - Style.space(13))
                            text: String(modelData.title || "Untitled")
                            textFormat: Text.PlainText
                            elide: Text.ElideRight
                            color: root.contentForeground
                            font.family: root.contentFontFamily
                            font.pixelSize: Style.font.body
                            font.bold: true
                          }
                        }

                        Text {
                          width: parent.width
                          text: agendaItemContent.parent.openingItem ? "Opening event…" : root.agendaMeta(modelData)
                          textFormat: Text.PlainText
                          color: agendaItemContent.parent.openingItem
                            ? Style.selectedStateColor(root.contentForeground, agendaItemContent.parent.sourceColor)
                            : Qt.darker(root.contentForeground, 1.5)
                          font.family: root.contentFontFamily
                          font.pixelSize: Style.font.caption
                          wrapMode: Text.Wrap
                          maximumLineCount: 2
                          elide: Text.ElideRight
                        }
                      }
                    }

                    MouseArea {
                      id: agendaItemMouse
                      anchors.fill: parent
                      hoverEnabled: true
                      cursorShape: parent.openingItem ? Qt.BusyCursor : Qt.PointingHandCursor
                      onClicked: {
                        if (!parent.openingItem) root.openAgendaItem(modelData)
                      }
                    }

                    PanelToolTip {
                      visible: agendaItemMouse.containsMouse
                      text: parent.openingItem ? "Opening event…" : String(modelData.title || "Untitled")
                      fontFamily: root.contentFontFamily
                    }
                  }
                }

                Text {
                  visible: root.actualService !== null
                  width: parent.width
                  topPadding: Style.space(3)
                  text: root.serviceStatus === "offline" ? "Cached offline · Tasks are date-only" : "Tasks are date-only"
                  textFormat: Text.PlainText
                  color: Qt.darker(root.contentForeground, 1.65)
                  font.family: root.contentFontFamily
                  font.pixelSize: Style.font.caption
                  horizontalAlignment: Text.AlignRight
                }
              }
            }
          }
        }
      }
    }
  }
}
