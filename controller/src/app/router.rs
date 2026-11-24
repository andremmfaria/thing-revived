//! The single command match: one `ClientCommand` in, service calls and events
//! out. Which thread runs it is decided by `policy::execution`.

use crate::app::hub::{ConnId, TargetedSink};
use crate::app::policy::{execution, Execution};
use crate::domain::accounts::password::Secret;
use crate::protocol::{BridgeEvent, ClientCommand};
use crate::services::accounts::AccountsService;
use crate::services::bluetooth::BluetoothService;
use crate::services::display::DisplayService;
use crate::services::timezone::TimezoneService;
use crate::services::usb_role::UsbRoleService;
use crate::services::users::UsersService;
use crate::services::wifi::Wifi;
use std::sync::{Arc, OnceLock};
use std::thread;

pub struct Router {
    bt: Arc<BluetoothService>,
    wifi: Arc<Wifi>,
    display: DisplayService,
    timezone: TimezoneService,
    /// Where replies to one connection go. Attached once by the WebSocket
    /// server at startup; unset only in tests that do not use it.
    replies: OnceLock<Arc<dyn TargetedSink>>,
    /// The first-boot password service. Attached at startup; while unset
    /// (tests that do not need it) `user_bootstrap_get` answers
    /// `internal`.
    users: OnceLock<Arc<UsersService>>,
    /// The account menu service (unlock, users, groups, keys). While unset,
    /// its commands answer `internal`.
    accounts: OnceLock<Arc<AccountsService>>,
    /// The USB role service. While unset (tests that do not need it) its
    /// commands answer `usb_role_error` `unavailable`.
    usb_role: OnceLock<Arc<UsbRoleService>>,
}

impl Router {
    pub fn new(bt: Arc<BluetoothService>, wifi: Arc<Wifi>, display: DisplayService, timezone: TimezoneService) -> Router {
        Router { bt, wifi, display, timezone, replies: OnceLock::new(), users: OnceLock::new(), accounts: OnceLock::new(), usb_role: OnceLock::new() }
    }

    /// Sets the sink for targeted replies. The first call wins.
    pub fn attach_replies(&self, sink: Arc<dyn TargetedSink>) {
        let _ = self.replies.set(sink);
    }

    /// Sets the user service. The first call wins.
    pub fn attach_users(&self, users: Arc<UsersService>) {
        let _ = self.users.set(users);
    }

    /// Sets the account menu service. The first call wins.
    pub fn attach_accounts(&self, accounts: Arc<AccountsService>) {
        let _ = self.accounts.set(accounts);
    }

    /// Sets the USB role service. The first call wins.
    pub fn attach_usb_role(&self, usb_role: Arc<UsbRoleService>) {
        let _ = self.usb_role.set(usb_role);
    }

    /// A websocket connection ended: the first-boot gate and the account
    /// menu forget it (its unlock session ends with it).
    pub fn connection_closed(&self, conn: ConnId) {
        if let Some(users) = self.users.get() {
            users.connection_closed(conn);
        }
        if let Some(accounts) = self.accounts.get() {
            accounts.connection_closed(conn);
        }
    }

    fn reply(&self, conn: ConnId, e: BridgeEvent) {
        match self.replies.get() {
            Some(sink) => {
                sink.emit_to(conn, e);
            }
            None => log::error!("targeted reply dropped: no reply sink attached"),
        }
    }

    /// Entry point for the reader thread: runs the command inline or hands it
    /// to a worker thread according to the execution policy.
    pub fn handle(self: &Arc<Self>, conn: ConnId, cmd: ClientCommand) {
        match execution(&cmd) {
            Execution::Inline => self.execute(conn, cmd),
            Execution::Blocking => {
                let router = self.clone();
                thread::spawn(move || router.execute(conn, cmd));
            }
        }
    }

    /// Applies one command on the calling thread. Every arm is self-contained
    /// and never panics on bad or out-of-range input: a malformed or hostile
    /// WS message must degrade to an error reply, not take the daemon down.
    pub fn execute(&self, conn: ConnId, cmd: ClientCommand) {
        match cmd {
            ClientCommand::BtPowerGet {} => self.bt.power_get(),
            ClientCommand::BtSetPower { powered } => self.bt.set_power(powered),
            ClientCommand::BtScanStart {} => self.bt.scan_start(),
            ClientCommand::BtScanStop {} => self.bt.scan_stop(),
            ClientCommand::BtPair { address } => self.bt.pair(&address),
            ClientCommand::BtPairingConfirm { accept } => self.bt.confirm(accept),
            ClientCommand::BtPairedListGet {} => self.bt.paired_list(),
            ClientCommand::BtConnect { address } => self.bt.connect(&address),
            ClientCommand::BtDisconnect { address } => self.bt.disconnect(&address),
            ClientCommand::BtForget { address } => self.bt.forget(&address),
            ClientCommand::BrightnessGet {} => self.display.get(),
            ClientCommand::BrightnessSet { value } => self.display.set(value),
            ClientCommand::TimezoneGet {} => self.timezone.get(),
            ClientCommand::TimezoneSet { value } => self.timezone.set(value),
            ClientCommand::TimezoneListGet {} => self.timezone.list(),
            ClientCommand::UsbRoleGet {} => self.usb_role(conn, "usb_role_get", |u| u.get()),
            ClientCommand::UsbRoleSet { role } => self.usb_role(conn, "usb_role_set", |u| u.set(&role)),
            ClientCommand::UsbRoleReboot {} => self.usb_role(conn, "usb_role_reboot", |u| u.reboot()),
            ClientCommand::WifiGet {} => self.wifi.announce(),
            ClientCommand::WifiScan {} => self.wifi.scan(),
            ClientCommand::WifiConnect { ssid, password } => self.wifi.connect(ssid, password.expose().to_string()),
            ClientCommand::WifiConnectSaved { ssid } => self.wifi.connect_saved(ssid),
            ClientCommand::WifiDisconnect {} => self.wifi.disconnect(),
            ClientCommand::WifiSavedListGet {} => self.wifi.list_saved(),
            ClientCommand::WifiForget { ssid } => self.wifi.forget(&ssid),
            // Every user_* reply goes to the requesting connection only. The
            // first-boot service answers `get` itself (phase, and the secret
            // while showing); generation and acknowledgement are the physical
            // M button, never a command.
            ClientCommand::UserBootstrapGet {} => match self.users.get() {
                Some(users) => users.bootstrap_get(conn),
                None => self.unavailable(conn, "user_bootstrap_get"),
            },
            ClientCommand::UserUnlock { username, password } => {
                self.accounts(conn, "user_unlock", |a| a.unlock(conn, &username, &Secret::new(password.expose())))
            }
            ClientCommand::UserLockSession {} => self.accounts(conn, "user_lock_session", |a| a.lock_session(conn)),
            ClientCommand::UserListGet {} => self.accounts(conn, "user_list_get", |a| a.list_users(conn)),
            ClientCommand::UserAdd { name, password, admin } => {
                self.accounts(conn, "user_add", |a| a.add_user(conn, &name, &Secret::new(password.expose()), admin))
            }
            ClientCommand::UserRemove { name, keep_home, confirm } => {
                self.accounts(conn, "user_remove", |a| a.remove_user(conn, &name, keep_home, &confirm))
            }
            ClientCommand::UserSetLocked { name, locked, confirm } => {
                self.accounts(conn, "user_set_locked", |a| a.set_locked(conn, &name, locked, &confirm))
            }
            ClientCommand::UserSetPassword { name, password, current_password, confirm } => self.accounts(conn, "user_set_password", |a| {
                a.set_password(conn, &name, &Secret::new(password.expose()), &Secret::new(current_password.expose()), &confirm)
            }),
            ClientCommand::GroupListGet {} => self.accounts(conn, "group_list_get", |a| a.list_groups(conn)),
            ClientCommand::UserGroupAdd { name, group, confirm } => {
                self.accounts(conn, "user_group_add", |a| a.add_member(conn, &name, &group, &confirm))
            }
            ClientCommand::UserGroupRemove { name, group } => {
                self.accounts(conn, "user_group_remove", |a| a.remove_member(conn, &name, &group))
            }
            ClientCommand::GroupAdd { name } => self.accounts(conn, "group_add", |a| a.add_group(conn, &name)),
            ClientCommand::GroupRemove { name, confirm } => self.accounts(conn, "group_remove", |a| a.remove_group(conn, &name, &confirm)),
            ClientCommand::SshKeyListGet { name } => self.accounts(conn, "ssh_key_list_get", |a| a.list_keys(conn, &name)),
            ClientCommand::SshKeyAdd { name, key } => self.accounts(conn, "ssh_key_add", |a| a.add_key(conn, &name, &key)),
            ClientCommand::SshKeyRemove { name, fingerprint, confirm } => {
                self.accounts(conn, "ssh_key_remove", |a| a.remove_key(conn, &name, &fingerprint, &confirm))
            }
        }
    }

    /// Runs `f` on the USB role service, or answers that it is missing.
    fn usb_role(&self, conn: ConnId, context: &str, f: impl FnOnce(&UsbRoleService)) {
        match self.usb_role.get() {
            Some(u) => f(u),
            None => self.reply(
                conn,
                BridgeEvent::UsbRoleError { context: context.into(), code: "unavailable".into(), message: "USB role control is not available".into() },
            ),
        }
    }

    /// Runs `f` on the account menu service, or answers that it is missing.
    fn accounts(&self, conn: ConnId, context: &str, f: impl FnOnce(&AccountsService)) {
        match self.accounts.get() {
            Some(accounts) => f(accounts),
            None => self.unavailable(conn, context),
        }
    }

    fn unavailable(&self, conn: ConnId, context: &str) {
        self.reply(
            conn,
            BridgeEvent::UserError {
                context: context.into(),
                code: "internal".into(),
                message: "User management is not available".into(),
            },
        );
    }
}
