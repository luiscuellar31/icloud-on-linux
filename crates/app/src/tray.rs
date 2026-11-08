//! The tray icon: a StatusNotifierItem with a `com.canonical.dbusmenu` menu, exported with GIO's D-Bus API. KDE Plasma
//! and the AppIndicator extension of GNOME show it through their StatusNotifierWatcher. The icon registers whenever a
//! watcher appears, as when the panel restarts; while none runs, it waits quietly.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::gio;
use gtk::glib::variant::{ObjectPath, ToVariant};
use gtk::glib::{self, Variant};

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const ITEM: &str = "org.kde.StatusNotifierItem";
const MENU: &str = "com.canonical.dbusmenu";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const INVALID: &str = "org.freedesktop.DBus.Error.InvalidArgs";

const INTERFACES: &str = r#"<node>
  <interface name="org.kde.StatusNotifierItem">
    <property name="Category" type="s" access="read"/>
    <property name="Id" type="s" access="read"/>
    <property name="Title" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="WindowId" type="i" access="read"/>
    <property name="IconThemePath" type="s" access="read"/>
    <property name="IconName" type="s" access="read"/>
    <property name="IconPixmap" type="a(iiay)" access="read"/>
    <property name="OverlayIconName" type="s" access="read"/>
    <property name="OverlayIconPixmap" type="a(iiay)" access="read"/>
    <property name="AttentionIconName" type="s" access="read"/>
    <property name="AttentionIconPixmap" type="a(iiay)" access="read"/>
    <property name="AttentionMovieName" type="s" access="read"/>
    <property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
    <property name="ItemIsMenu" type="b" access="read"/>
    <property name="Menu" type="o" access="read"/>
    <method name="ContextMenu"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
    <method name="Activate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
    <method name="SecondaryActivate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
    <method name="Scroll"><arg name="delta" type="i" direction="in"/><arg name="orientation" type="s" direction="in"/></method>
    <method name="ProvideXdgActivationToken"><arg name="token" type="s" direction="in"/></method>
    <signal name="NewTitle"/>
    <signal name="NewIcon"/>
    <signal name="NewAttentionIcon"/>
    <signal name="NewOverlayIcon"/>
    <signal name="NewToolTip"/>
    <signal name="NewStatus"><arg name="status" type="s"/></signal>
  </interface>
  <interface name="com.canonical.dbusmenu">
    <property name="Version" type="u" access="read"/>
    <property name="TextDirection" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="IconThemePath" type="as" access="read"/>
    <method name="GetLayout">
      <arg name="parentId" type="i" direction="in"/>
      <arg name="recursionDepth" type="i" direction="in"/>
      <arg name="propertyNames" type="as" direction="in"/>
      <arg name="revision" type="u" direction="out"/>
      <arg name="layout" type="(ia{sv}av)" direction="out"/>
    </method>
    <method name="GetGroupProperties">
      <arg name="ids" type="ai" direction="in"/>
      <arg name="propertyNames" type="as" direction="in"/>
      <arg name="properties" type="a(ia{sv})" direction="out"/>
    </method>
    <method name="GetProperty">
      <arg name="id" type="i" direction="in"/>
      <arg name="name" type="s" direction="in"/>
      <arg name="value" type="v" direction="out"/>
    </method>
    <method name="Event">
      <arg name="id" type="i" direction="in"/>
      <arg name="eventId" type="s" direction="in"/>
      <arg name="data" type="v" direction="in"/>
      <arg name="timestamp" type="u" direction="in"/>
    </method>
    <method name="EventGroup">
      <arg name="events" type="a(isvu)" direction="in"/>
      <arg name="idErrors" type="ai" direction="out"/>
    </method>
    <method name="AboutToShow">
      <arg name="id" type="i" direction="in"/>
      <arg name="needUpdate" type="b" direction="out"/>
    </method>
    <method name="AboutToShowGroup">
      <arg name="ids" type="ai" direction="in"/>
      <arg name="updatesNeeded" type="ai" direction="out"/>
      <arg name="idErrors" type="ai" direction="out"/>
    </method>
    <signal name="ItemsPropertiesUpdated">
      <arg name="updatedProps" type="a(ia{sv})"/>
      <arg name="removedProps" type="a(ias)"/>
    </signal>
    <signal name="LayoutUpdated"><arg name="revision" type="u"/><arg name="parent" type="i"/></signal>
    <signal name="ItemActivationRequested"><arg name="id" type="i"/><arg name="timestamp" type="u"/></signal>
  </interface>
</node>"#;

type Properties = HashMap<String, Variant>;
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

/// A menu entry, as `add_item` returns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item(i32);

struct Entry {
    id: i32,
    /// `None` for a separator.
    label: Option<String>,
    enabled: bool,
    visible: bool,
    action: Option<Rc<dyn Fn()>>,
}

impl Entry {
    /// The dbusmenu properties of the entry, all of them or only those in `names`.
    fn properties(&self, names: &[String]) -> Properties {
        let mut properties = Properties::new();
        match &self.label {
            Some(label) => {
                // An underscore marks a mnemonic, so a literal one is doubled.
                properties.insert("label".into(), label.replace('_', "__").to_variant());
                properties.insert("enabled".into(), self.enabled.to_variant());
            }
            None => {
                properties.insert("type".into(), "separator".to_variant());
            }
        }
        properties.insert("visible".into(), self.visible.to_variant());
        properties.retain(|name, _| names.is_empty() || names.contains(name));
        properties
    }
}

pub struct Tray {
    connection: gio::DBusConnection,
    id: String,
    title: String,
    icon: String,
    tooltip: RefCell<String>,
    entries: RefCell<Vec<Entry>>,
    revision: Cell<u32>,
    activate: RefCell<Option<Rc<dyn Fn()>>>,
    token: RefCell<Option<String>>,
    shown: Cell<bool>,
    registrations: RefCell<Vec<gio::RegistrationId>>,
    // gio exports another type under the name of the watcher's id, so the tray keeps a closure that ends the watch.
    unwatch: RefCell<Option<Box<dyn FnOnce()>>>,
}

impl Tray {
    /// Export an icon on `connection`, normally the application's session bus connection: `id` names the app for the
    /// desktop, `title` is shown to the user, and `icon` is a themed icon name.
    pub fn new(connection: &gio::DBusConnection, id: &str, title: &str, icon: &str) -> Result<Rc<Tray>, glib::Error> {
        let node = gio::DBusNodeInfo::for_xml(INTERFACES)?;
        let tray = Rc::new(Tray {
            connection: connection.clone(),
            id: id.to_owned(),
            title: title.to_owned(),
            icon: icon.to_owned(),
            tooltip: RefCell::new(title.to_owned()),
            entries: RefCell::default(),
            revision: Cell::new(1),
            activate: RefCell::default(),
            token: RefCell::default(),
            shown: Cell::new(false),
            registrations: RefCell::default(),
            unwatch: RefCell::default(),
        });
        for (path, interface) in [(ITEM_PATH, ITEM), (MENU_PATH, MENU)] {
            let info = node.lookup_interface(interface).expect("the interface is described");
            let (calls, reads) = (Rc::downgrade(&tray), Rc::downgrade(&tray));
            let registration = connection
                .register_object(path, &info)
                .method_call(
                    move |_, _, _, _, method, parameters, invocation| match calls.upgrade() {
                        Some(tray) if interface == ITEM => tray.item_call(method, &parameters, invocation),
                        Some(tray) => tray.menu_call(method, &parameters, invocation),
                        None => {
                            invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownObject", "No tray icon")
                        }
                    },
                )
                .property(move |_, _, _, _, name| match reads.upgrade() {
                    Some(tray) if interface == ITEM => tray.item_property(name),
                    Some(tray) => tray.menu_property(name),
                    None => "".to_variant(),
                })
                .build()?;
            tray.registrations.borrow_mut().push(registration);
        }
        let (appeared, vanished) = (Rc::downgrade(&tray), Rc::downgrade(&tray));
        let watcher = gio::bus_watch_name_on_connection(
            connection,
            WATCHER,
            gio::BusNameWatcherFlags::NONE,
            move |_, _, _| {
                if let Some(tray) = appeared.upgrade() {
                    tray.register();
                }
            },
            move |_, _| {
                if let Some(tray) = vanished.upgrade() {
                    tray.shown.set(false);
                }
            },
        );
        *tray.unwatch.borrow_mut() = Some(Box::new(move || gio::bus_unwatch_name(watcher)));
        Ok(tray)
    }

    /// Add a menu entry that runs `action` when clicked while it is enabled and visible.
    pub fn add_item(&self, label: &str, action: impl Fn() + 'static) -> Item {
        self.push(Some(label.to_owned()), Some(Rc::new(action)))
    }

    pub fn add_separator(&self) {
        self.push(None, None);
    }

    pub fn set_label(&self, item: Item, label: &str) {
        self.update(item, |entry| {
            let changed = entry.label.as_deref() != Some(label);
            entry.label = Some(label.to_owned());
            changed
        });
    }

    pub fn set_enabled(&self, item: Item, enabled: bool) {
        self.update(item, |entry| std::mem::replace(&mut entry.enabled, enabled) != enabled);
    }

    pub fn set_visible(&self, item: Item, visible: bool) {
        self.update(item, |entry| std::mem::replace(&mut entry.visible, visible) != visible);
    }

    pub fn set_tooltip(&self, text: &str) {
        if self.tooltip.replace(text.to_owned()) != text {
            self.emit(ITEM_PATH, ITEM, "NewToolTip", None);
        }
    }

    /// Run `action` when the icon itself is clicked.
    pub fn connect_activate(&self, action: impl Fn() + 'static) {
        *self.activate.borrow_mut() = Some(Rc::new(action));
    }

    /// The token that the desktop sent with the last click, to raise the window on Wayland.
    pub fn take_activation_token(&self) -> Option<String> {
        self.token.take()
    }

    /// Whether a StatusNotifierWatcher accepted the icon, so the desktop can show it.
    pub fn shown(&self) -> bool {
        self.shown.get()
    }

    fn push(&self, label: Option<String>, action: Option<Rc<dyn Fn()>>) -> Item {
        let id = {
            let mut entries = self.entries.borrow_mut();
            let id = entries.len() as i32 + 1;
            entries.push(Entry {
                id,
                label,
                enabled: true,
                visible: true,
                action,
            });
            id
        };
        self.revision.set(self.revision.get() + 1);
        let changed = (self.revision.get(), 0i32).to_variant();
        self.emit(MENU_PATH, MENU, "LayoutUpdated", Some(&changed));
        Item(id)
    }

    /// Apply `change` to an entry and announce its properties if it reports a difference.
    fn update(&self, item: Item, change: impl FnOnce(&mut Entry) -> bool) {
        let properties = {
            let mut entries = self.entries.borrow_mut();
            let Some(entry) = entries.iter_mut().find(|entry| entry.id == item.0) else {
                return;
            };
            if !change(entry) {
                return;
            }
            entry.properties(&[])
        };
        let removed: Vec<(i32, Vec<String>)> = Vec::new();
        let updated = (vec![(item.0, properties)], removed).to_variant();
        self.emit(MENU_PATH, MENU, "ItemsPropertiesUpdated", Some(&updated));
    }

    fn register(self: &Rc<Self>) {
        // KDE accepts only a service that is already on the bus; the unique name is, unlike a requested name.
        let Some(name) = self.connection.unique_name() else {
            return;
        };
        let weak = Rc::downgrade(self);
        self.connection.call(
            Some(WATCHER),
            "/StatusNotifierWatcher",
            WATCHER,
            "RegisterStatusNotifierItem",
            Some(&(name.as_str(),).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            -1,
            gio::Cancellable::NONE,
            move |result| {
                if let Some(tray) = weak.upgrade() {
                    tray.shown.set(result.is_ok());
                }
            },
        );
    }

    fn emit(&self, path: &str, interface: &str, signal: &str, parameters: Option<&Variant>) {
        // Without a connection there is nobody left to tell.
        let _ = self.connection.emit_signal(None, path, interface, signal, parameters);
    }

    fn item_call(&self, method: &str, parameters: &Variant, invocation: gio::DBusMethodInvocation) {
        if method == "ProvideXdgActivationToken" {
            *self.token.borrow_mut() = parameters.child_value(0).str().map(str::to_owned);
        }
        invocation.return_value(None);
        // The menu holds every other action; the desktop shows it on a right click.
        if method == "Activate" {
            let action = self.activate.borrow().clone();
            if let Some(action) = action {
                action();
            }
        }
    }

    fn item_property(&self, name: &str) -> Variant {
        match name {
            "Category" => "ApplicationStatus".to_variant(),
            "Id" => self.id.to_variant(),
            "Title" => self.title.to_variant(),
            "Status" => "Active".to_variant(),
            "WindowId" => 0i32.to_variant(),
            "IconName" => self.icon.to_variant(),
            "IconPixmap" | "OverlayIconPixmap" | "AttentionIconPixmap" => Pixmaps::new().to_variant(),
            "ToolTip" => (self.icon.as_str(), Pixmaps::new(), self.tooltip.borrow().as_str(), "").to_variant(),
            "ItemIsMenu" => false.to_variant(),
            "Menu" => ObjectPath::try_from(MENU_PATH).expect("a valid path").to_variant(),
            _ => "".to_variant(),
        }
    }

    fn menu_property(&self, name: &str) -> Variant {
        match name {
            "Version" => 3u32.to_variant(),
            "TextDirection" => "ltr".to_variant(),
            "Status" => "normal".to_variant(),
            _ => Vec::<String>::new().to_variant(),
        }
    }

    fn menu_call(&self, method: &str, parameters: &Variant, invocation: gio::DBusMethodInvocation) {
        match method {
            "GetLayout" => {
                let (parent, depth, names) = parameters.get::<(i32, i32, Vec<String>)>().unwrap_or_default();
                match self.layout(parent, depth, &names) {
                    Some(layout) => invocation.return_value(Some(&(self.revision.get(), layout).to_variant())),
                    None => invocation.return_dbus_error(INVALID, "No such menu item"),
                }
            }
            "GetGroupProperties" => {
                let (ids, names) = parameters.get::<(Vec<i32>, Vec<String>)>().unwrap_or_default();
                let found: Vec<(i32, Properties)> = self
                    .entries
                    .borrow()
                    .iter()
                    .filter(|entry| ids.is_empty() || ids.contains(&entry.id))
                    .map(|entry| (entry.id, entry.properties(&names)))
                    .collect();
                invocation.return_value(Some(&(found,).to_variant()));
            }
            "GetProperty" => {
                let (id, name) = parameters.get::<(i32, String)>().unwrap_or_default();
                let names = [name];
                let value = self
                    .entries
                    .borrow()
                    .iter()
                    .find(|entry| entry.id == id)
                    .and_then(|entry| entry.properties(&names).remove(&names[0]));
                match value {
                    Some(value) => invocation.return_value(Some(&(value,).to_variant())),
                    None => invocation.return_dbus_error(INVALID, "No such menu item or property"),
                }
            }
            "Event" => {
                let id = parameters.child_value(0).get::<i32>().unwrap_or_default();
                let event = parameters.child_value(1).get::<String>().unwrap_or_default();
                let action = self.clicked(id, &event);
                invocation.return_value(None);
                if let Some(action) = action {
                    action();
                }
            }
            "EventGroup" => {
                let events = parameters
                    .child_value(0)
                    .get::<Vec<(i32, String, Variant, u32)>>()
                    .unwrap_or_default();
                let known: Vec<i32> = self.entries.borrow().iter().map(|entry| entry.id).collect();
                let errors: Vec<i32> = events
                    .iter()
                    .map(|(id, ..)| *id)
                    .filter(|id| !known.contains(id))
                    .collect();
                let actions: Vec<Rc<dyn Fn()>> = events
                    .iter()
                    .filter_map(|(id, event, ..)| self.clicked(*id, event))
                    .collect();
                invocation.return_value(Some(&(errors,).to_variant()));
                for action in actions {
                    action();
                }
            }
            "AboutToShow" => invocation.return_value(Some(&(false,).to_variant())),
            "AboutToShowGroup" => invocation.return_value(Some(&(Vec::<i32>::new(), Vec::<i32>::new()).to_variant())),
            _ => invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", "Unknown method"),
        }
    }

    /// The menu from `parent`, which is the root (0) or an entry, with its entries unless `depth` is 0.
    fn layout(&self, parent: i32, depth: i32, names: &[String]) -> Option<(i32, Properties, Vec<Variant>)> {
        let entries = self.entries.borrow();
        if parent != 0 {
            let entry = entries.iter().find(|entry| entry.id == parent)?;
            return Some((entry.id, entry.properties(names), Vec::new()));
        }
        let mut root = Properties::from([("children-display".to_owned(), "submenu".to_variant())]);
        root.retain(|name, _| names.is_empty() || names.contains(name));
        let children = match depth {
            0 => Vec::new(),
            _ => entries
                .iter()
                .map(|entry| (entry.id, entry.properties(names), Vec::<Variant>::new()).to_variant())
                .collect(),
        };
        Some((0, root, children))
    }

    /// The action of a click on an enabled, visible entry.
    fn clicked(&self, id: i32, event: &str) -> Option<Rc<dyn Fn()>> {
        let entries = self.entries.borrow();
        let entry = entries
            .iter()
            .find(|entry| entry.id == id && entry.enabled && entry.visible)?;
        (event == "clicked").then(|| entry.action.clone()).flatten()
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        if let Some(unwatch) = self.unwatch.take() {
            unwatch();
        }
        for registration in self.registrations.take() {
            let _ = self.connection.unregister_object(registration);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    const WATCHER_XML: &str = r#"<node><interface name="org.kde.StatusNotifierWatcher">
      <method name="RegisterStatusNotifierItem"><arg name="service" type="s" direction="in"/></method>
    </interface></node>"#;

    /// A private message bus, so test icons never reach the desktop session.
    struct Bus(Child, String);

    impl Bus {
        fn start() -> Option<Bus> {
            let mut daemon = Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .ok()?;
            let mut address = String::new();
            BufReader::new(daemon.stdout.take()?).read_line(&mut address).ok()?;
            let bus = Bus(daemon, address.trim().to_owned());
            (!bus.1.is_empty()).then_some(bus)
        }

        fn connect(&self) -> gio::DBusConnection {
            let flags =
                gio::DBusConnectionFlags::AUTHENTICATION_CLIENT | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION;
            gio::DBusConnection::for_address_sync(&self.1, flags, None, gio::Cancellable::NONE).unwrap()
        }
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// A StatusNotifierWatcher that records the services registering with it.
    struct Watcher {
        connection: gio::DBusConnection,
        registration: Option<gio::RegistrationId>,
        owner: Option<gio::OwnerId>,
    }

    impl Watcher {
        fn start(connection: &gio::DBusConnection, registered: Rc<RefCell<Vec<String>>>) -> Watcher {
            let node = gio::DBusNodeInfo::for_xml(WATCHER_XML).unwrap();
            let info = node.lookup_interface(WATCHER).unwrap();
            let registration = connection
                .register_object("/StatusNotifierWatcher", &info)
                .method_call(move |_, _, _, _, _, parameters, invocation| {
                    registered
                        .borrow_mut()
                        .extend(parameters.child_value(0).str().map(str::to_owned));
                    invocation.return_value(None);
                })
                .build()
                .unwrap();
            let owner = gio::bus_own_name_on_connection(
                connection,
                WATCHER,
                gio::BusNameOwnerFlags::NONE,
                |_, _| {},
                |_, _| {},
            );
            Watcher {
                connection: connection.clone(),
                registration: Some(registration),
                owner: Some(owner),
            }
        }
    }

    impl Drop for Watcher {
        fn drop(&mut self) {
            if let Some(owner) = self.owner.take() {
                gio::bus_unown_name(owner);
            }
            if let Some(registration) = self.registration.take() {
                let _ = self.connection.unregister_object(registration);
            }
        }
    }

    fn wait(context: &glib::MainContext, what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            while context.iteration(false) {}
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Let pending messages arrive when nothing should happen.
    fn settle(context: &glib::MainContext) {
        for _ in 0..50 {
            while context.iteration(false) {}
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    struct Host<'a> {
        context: &'a glib::MainContext,
        connection: gio::DBusConnection,
        tray: String,
    }

    impl Host<'_> {
        fn call(&self, path: &str, interface: &str, method: &str, parameters: Variant) -> Variant {
            let reply = Rc::new(RefCell::new(None));
            let slot = reply.clone();
            self.connection.call(
                Some(&self.tray),
                path,
                interface,
                method,
                Some(&parameters),
                None,
                gio::DBusCallFlags::NONE,
                5000,
                gio::Cancellable::NONE,
                move |result| *slot.borrow_mut() = Some(result),
            );
            wait(self.context, method, || reply.borrow().is_some());
            let result = reply.take().unwrap();
            result.unwrap_or_else(|error| panic!("{method} failed: {error}"))
        }

        fn get(&self, interface: &str, name: &str) -> Variant {
            let path = if interface == ITEM { ITEM_PATH } else { MENU_PATH };
            let reply = self.call(
                path,
                "org.freedesktop.DBus.Properties",
                "Get",
                (interface, name).to_variant(),
            );
            reply.child_value(0).as_variant().unwrap()
        }

        fn click(&self, item: i32) {
            self.call(
                MENU_PATH,
                MENU,
                "Event",
                (item, "clicked", "".to_variant(), 0u32).to_variant(),
            );
        }

        fn labels(&self) -> Vec<String> {
            let layout = self.call(
                MENU_PATH,
                MENU,
                "GetLayout",
                (0i32, -1i32, Vec::<String>::new()).to_variant(),
            );
            let (_, (root, _, children)) = layout.get::<(u32, (i32, Properties, Vec<Variant>))>().unwrap();
            assert_eq!(root, 0);
            children
                .iter()
                .map(|child| {
                    let (_, properties, _) = child.get::<(i32, Properties, Vec<Variant>)>().unwrap();
                    let enabled = properties.get("enabled").and_then(Variant::get::<bool>) != Some(false);
                    match properties.get("label").and_then(Variant::str) {
                        Some(label) if enabled => label.to_owned(),
                        Some(label) => format!("({label})"),
                        None => "-".to_owned(),
                    }
                })
                .collect()
        }
    }

    #[test]
    fn the_icon_registers_with_a_watcher_and_its_menu_reaches_the_actions() {
        let Some(bus) = Bus::start() else {
            println!("SKIP: the tray check needs dbus-daemon for a private bus.");
            return;
        };
        let context = glib::MainContext::new();
        context.with_thread_default(|| check(&context, &bus)).unwrap();
    }

    fn check(context: &glib::MainContext, bus: &Bus) {
        let app = bus.connect();
        let tray = Tray::new(&app, "icloud-on-linux", "iCloud on Linux", "folder-remote").unwrap();
        let clicked = Rc::new(RefCell::new(Vec::new()));
        let record = |name: &'static str| {
            let clicked = clicked.clone();
            move || clicked.borrow_mut().push(name)
        };
        // The menu of the Python app.
        let show = tray.add_item("Show window", record("show"));
        let open = tray.add_item("Open iCloud Drive", record("open"));
        let sync = tray.add_item("Sync now", record("sync"));
        tray.add_separator();
        tray.add_item("Quit", record("quit"));
        tray.connect_activate(record("activate"));

        // Without a watcher nothing is registered; one that starts later gets the icon.
        settle(context);
        assert!(!tray.shown());
        let host = Host {
            context,
            connection: bus.connect(),
            tray: app.unique_name().unwrap().to_string(),
        };
        let registered = Rc::new(RefCell::new(Vec::new()));
        let watcher = Watcher::start(&host.connection, registered.clone());
        wait(context, "the icon registers", || tray.shown());
        assert_eq!(registered.borrow().as_slice(), std::slice::from_ref(&host.tray));

        assert_eq!(host.get(ITEM, "IconName").str(), Some("folder-remote"));
        assert_eq!(host.get(ITEM, "Title").str(), Some("iCloud on Linux"));
        assert_eq!(host.get(ITEM, "Id").str(), Some("icloud-on-linux"));
        assert_eq!(host.get(ITEM, "Menu").str(), Some(MENU_PATH));
        assert_eq!(host.get(MENU, "Version").get::<u32>(), Some(3));
        // Every declared property has a value of its declared type.
        let all = host.call(
            ITEM_PATH,
            "org.freedesktop.DBus.Properties",
            "GetAll",
            (ITEM,).to_variant(),
        );
        assert_eq!(all.child_value(0).n_children(), 16);
        assert_eq!(
            host.labels(),
            ["Show window", "Open iCloud Drive", "Sync now", "-", "Quit"]
        );

        host.click(open.0);
        host.call(
            ITEM_PATH,
            ITEM,
            "ProvideXdgActivationToken",
            ("fixture-token",).to_variant(),
        );
        host.call(ITEM_PATH, ITEM, "Activate", (0i32, 0i32).to_variant());
        assert_eq!(*clicked.borrow(), ["open", "activate"]);
        assert_eq!(tray.take_activation_token().as_deref(), Some("fixture-token"));

        let signals = Rc::new(RefCell::new(Vec::new()));
        let seen = signals.clone();
        let _subscription = host.connection.subscribe_to_signal(
            Some(&host.tray),
            None,
            None,
            None,
            None,
            gio::DBusSignalFlags::NONE,
            move |signal| {
                seen.borrow_mut()
                    .push((signal.signal_name.to_owned(), signal.parameters.clone()))
            },
        );
        // A call after subscribing makes the bus apply the subscription before the changes.
        host.get(ITEM, "Status");
        tray.set_tooltip("Connected");
        tray.set_label(show, "Mostrar ventana");
        tray.set_enabled(sync, false);
        tray.set_enabled(sync, false);
        wait(context, "the changes are announced", || signals.borrow().len() >= 3);
        settle(context);
        let names: Vec<String> = signals.borrow().iter().map(|(name, _)| name.clone()).collect();
        assert_eq!(
            names,
            ["NewToolTip", "ItemsPropertiesUpdated", "ItemsPropertiesUpdated"]
        );
        let (updated, removed) = signals.borrow()[1]
            .1
            .get::<(Vec<(i32, Properties)>, Vec<(i32, Vec<String>)>)>()
            .unwrap();
        assert_eq!((updated[0].0, removed.len()), (show.0, 0));
        assert_eq!(updated[0].1["label"].str(), Some("Mostrar ventana"));
        let tooltip = host.get(ITEM, "ToolTip");
        assert_eq!(tooltip.child_value(2).str(), Some("Connected"));
        assert_eq!(
            host.labels(),
            ["Mostrar ventana", "Open iCloud Drive", "(Sync now)", "-", "Quit"]
        );
        // A disabled or unknown entry does nothing when clicked.
        host.click(sync.0);
        host.click(99);
        assert_eq!(*clicked.borrow(), ["open", "activate"]);

        // A watcher that starts again, as after a panel restart, gets the icon again.
        drop(watcher);
        wait(context, "the icon notices the watcher left", || !tray.shown());
        let _watcher = Watcher::start(&host.connection, registered.clone());
        wait(context, "the icon registers again", || {
            tray.shown() && registered.borrow().len() == 2
        });
    }
}
