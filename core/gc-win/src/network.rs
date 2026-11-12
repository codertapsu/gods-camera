//! What the scanner's hub needs from Windows' networking: the computer's
//! name, an announcement on the local network so that phones and tablets
//! find the hub, and what Windows Firewall will do when one of them calls.

use std::ffi::c_void;
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use windows::core::{BSTR, HSTRING, PCWSTR, PWSTR};
use windows::Win32::NetworkManagement::Dns::{
    DnsServiceConstructInstance, DnsServiceDeRegister, DnsServiceFreeInstance, DnsServiceRegister, DNS_QUERY_REQUEST_VERSION1,
    DNS_SERVICE_INSTANCE, DNS_SERVICE_REGISTER_REQUEST,
};
use windows::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
    IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::NetworkManagement::WindowsFirewall::{
    INetFwPolicy2, INetFwRule, NetFwPolicy2, NET_FW_ACTION, NET_FW_ACTION_ALLOW, NET_FW_ACTION_BLOCK, NET_FW_IP_PROTOCOL_TCP,
    NET_FW_PROFILE2_PRIVATE, NET_FW_PROFILE2_PUBLIC, NET_FW_RULE_DIR_IN,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};
use windows::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};

/// The kind of service the hub announces itself as. The apps on phones and
/// tablets look for this, as they do for the Mac's hub.
pub const SERVICE_TYPE: &str = "_godscamera._tcp";

/// The firewall rule that lets devices on a private network reach the hub,
/// and the one that settles public networks without Windows having to ask.
/// `tools\allow-windows-hub.ps1` makes both.
pub const RULE_PRIVATE: &str = "God's Camera - devices on a private network";
pub const RULE_PUBLIC: &str = "God's Camera - nothing from public networks";

/// This computer's name, as Windows shows it under Settings > System > About.
pub fn computer_name() -> String {
    let mut length = 0u32;
    // The first call says how long the name is; it "fails" by design.
    let _ = unsafe { GetComputerNameExW(ComputerNameDnsHostname, None, &mut length) };
    let mut name = vec![0u16; length as usize + 1];
    let mut length = name.len() as u32;
    match unsafe { GetComputerNameExW(ComputerNameDnsHostname, Some(PWSTR(name.as_mut_ptr())), &mut length) } {
        Ok(()) => String::from_utf16_lossy(&name[..length as usize]),
        Err(_) => String::new(),
    }
}

/// This computer's addresses on the networks it reaches others through:
/// those of connected adapters that have a way out, which leaves out the
/// adapters of virtual machines. Nothing is sent to find them out. A device
/// that cannot find the hub by itself can be given one of these.
pub fn addresses() -> Vec<Ipv4Addr> {
    const ERROR_BUFFER_OVERFLOW: u32 = 111;
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    // Eight-byte words, so that the list Windows writes is aligned as its structures need.
    let mut size = 16 * 1024u32;
    let mut list = vec![0u64; size as usize / 8];
    let mut result = unsafe { GetAdaptersAddresses(AF_INET.0 as u32, flags, None, Some(list.as_mut_ptr().cast()), &mut size) };
    if result == ERROR_BUFFER_OVERFLOW {
        list = vec![0u64; size as usize / 8 + 1];
        result = unsafe { GetAdaptersAddresses(AF_INET.0 as u32, flags, None, Some(list.as_mut_ptr().cast()), &mut size) };
    }
    let mut found = Vec::new();
    if result != 0 {
        return found;
    }
    let mut adapter = list.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !adapter.is_null() {
        let this = unsafe { &*adapter };
        if this.OperStatus == IfOperStatusUp && !this.FirstGatewayAddress.is_null() {
            let mut next = this.FirstUnicastAddress;
            while !next.is_null() {
                let address = unsafe { &*next };
                let socket = address.Address.lpSockaddr;
                if !socket.is_null() && unsafe { (*socket).sa_family } == AF_INET {
                    let v4 = unsafe { &*(socket as *const SOCKADDR_IN) };
                    // Kept as it travels: its four bytes in order.
                    found.push(Ipv4Addr::from(unsafe { v4.sin_addr.S_un.S_addr }.to_ne_bytes()));
                }
                next = address.Next;
            }
        }
        adapter = this.Next;
    }
    found
}

/// Where an announcement stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Announced {
    /// Asked for; Windows has not answered yet.
    Starting,
    /// Devices on the network can find the hub.
    Up,
    /// Windows could not announce it; its own code says why.
    Failed(u32),
    /// Taken back.
    Down,
}

/// What Windows and its callback share while an announcement lasts.
struct Shared {
    state: Mutex<Announced>,
    changed: Condvar,
}

/// The hub's announcement on the local network: "there is a God's Camera
/// hub on this computer, at this port". Windows itself sends it, with the
/// service that answers name questions on the local network (multicast DNS,
/// which Apple calls Bonjour), so this program opens no port for it.
///
/// It lasts until this is dropped.
pub struct Announcement {
    shared: Arc<Shared>,
    request: Box<DNS_SERVICE_REGISTER_REQUEST>,
    instance: *mut DNS_SERVICE_INSTANCE,
}

// The request and the instance are only handed to Windows, which may use them from any thread.
unsafe impl Send for Announcement {}
unsafe impl Sync for Announcement {}

/// Windows' answer when a request was taken and its result will follow.
const DNS_REQUEST_PENDING: u32 = 9506;

/// Called by Windows when an announcement is up, has failed or was taken back.
unsafe extern "system" fn completed(status: u32, context: *const c_void, instance: *const DNS_SERVICE_INSTANCE) {
    if !instance.is_null() {
        // The copy Windows hands over is ours to free.
        unsafe { DnsServiceFreeInstance(instance) };
    }
    if context.is_null() {
        return;
    }
    let shared = unsafe { &*(context as *const Shared) };
    let mut state = shared.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *state = match (*state, status) {
        (Announced::Starting, 0) => Announced::Up,
        (Announced::Starting, code) => Announced::Failed(code),
        // The second call answers the request to take it back.
        _ => Announced::Down,
    };
    shared.changed.notify_all();
}

impl Announcement {
    /// Announce a hub named `name` at `port`. `fingerprint` is the hub
    /// certificate's; its start lets a device pick its own computer out of
    /// several. The same three text entries as the Mac's hub announces.
    pub fn start(name: &str, port: u16, fingerprint: &str) -> Result<Self, String> {
        let host = computer_name();
        if name.is_empty() || host.is_empty() {
            return Err("this computer has no name to announce".into());
        }
        // A dot inside the name would be read as the end of it.
        let service = HSTRING::from(format!("{}.{SERVICE_TYPE}.local", name.replace('\\', "\\\\").replace('.', "\\.")));
        let host = HSTRING::from(format!("{host}.local"));
        let short: String = fingerprint.chars().take(16).collect();
        let keys = [HSTRING::from("v"), HSTRING::from("name"), HSTRING::from("fp")];
        let values = [HSTRING::from("1"), HSTRING::from(name), HSTRING::from(short)];
        let key_pointers: Vec<PCWSTR> = keys.iter().map(|key| PCWSTR(key.as_ptr())).collect();
        let value_pointers: Vec<PCWSTR> = values.iter().map(|value| PCWSTR(value.as_ptr())).collect();
        // Without addresses, Windows announces the ones this computer has.
        let instance = unsafe {
            DnsServiceConstructInstance(&service, &host, None, None, port, 0, 0, keys.len() as u32, key_pointers.as_ptr(), value_pointers.as_ptr())
        };
        if instance.is_null() {
            return Err("Windows could not describe the hub for the network".into());
        }
        let shared = Arc::new(Shared { state: Mutex::new(Announced::Starting), changed: Condvar::new() });
        let request = Box::new(DNS_SERVICE_REGISTER_REQUEST {
            Version: DNS_QUERY_REQUEST_VERSION1.0,
            // On every network this computer is on.
            InterfaceIndex: 0,
            pServiceInstance: instance,
            pRegisterCompletionCallback: Some(completed),
            pQueryContext: Arc::as_ptr(&shared) as *mut c_void,
            ..Default::default()
        });
        let status = unsafe { DnsServiceRegister(&*request, None) };
        if status != DNS_REQUEST_PENDING {
            unsafe { DnsServiceFreeInstance(instance) };
            return Err(format!("Windows refused to announce the hub ({status})"));
        }
        Ok(Self { shared, request, instance })
    }

    pub fn state(&self) -> Announced {
        *self.shared.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Wait until Windows has answered, or `longest` has passed.
    pub fn settled(&self, longest: Duration) -> Announced {
        let state = self.shared.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (state, _) = self
            .shared
            .changed
            .wait_timeout_while(state, longest, |state| *state == Announced::Starting)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *state
    }
}

impl Drop for Announcement {
    fn drop(&mut self) {
        // An announcement that never came up has nothing to take back, once Windows has said so.
        let state = self.settled(Duration::from_secs(2));
        let mut finished = matches!(state, Announced::Failed(_));
        if !finished && unsafe { DnsServiceDeRegister(&*self.request, None) } == DNS_REQUEST_PENDING {
            let state = self.shared.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let (state, _) = self
                .shared
                .changed
                .wait_timeout_while(state, Duration::from_secs(3), |state| *state != Announced::Down)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            finished = *state == Announced::Down;
        }
        if finished {
            unsafe { DnsServiceFreeInstance(self.instance) };
        } else {
            // Windows may still call back with these: better to leave them than to free them under it.
            std::mem::forget(Arc::clone(&self.shared));
            let request = std::mem::take(&mut self.request);
            std::mem::forget(request);
        }
    }
}

/// What Windows Firewall will do when a device on the network calls the hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FirewallAnswer {
    /// The rule that lets devices on a private network in is there, for this program and port.
    pub private_allowed: bool,
    /// Public networks are settled by a rule, so Windows will not ask the user about them.
    pub public_settled: bool,
    /// This computer is on a network that Windows treats as private, such as home Wi-Fi.
    pub on_private_network: bool,
    /// This computer is on a network that Windows treats as public. Virtual
    /// network adapters count, so this is often true beside a private one.
    pub on_public_network: bool,
}

impl FirewallAnswer {
    /// A hub that listens on the network now reaches the devices it should,
    /// and Windows shows no question about it.
    pub fn ready(&self) -> bool {
        self.private_allowed && (self.public_settled || !self.on_public_network)
    }
}

/// Ask Windows Firewall about `program` listening on TCP `port`. Reading the
/// rules needs no administrator.
pub fn firewall(program: &Path, port: u16) -> Result<FirewallAnswer, String> {
    let program = program.to_path_buf();
    // On a thread of its own: the caller's may have set COM up another way.
    std::thread::spawn(move || {
        let started = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let answer = unsafe { read_firewall(&program, port) }.map_err(|error| format!("Windows Firewall answered {:#010x}", error.code().0));
        if started.is_ok() {
            unsafe { CoUninitialize() };
        }
        answer
    })
    .join()
    .map_err(|_| "asking Windows Firewall failed".to_string())?
}

unsafe fn read_firewall(program: &Path, port: u16) -> windows::core::Result<FirewallAnswer> {
    let policy: INetFwPolicy2 = unsafe { CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER) }?;
    let active = unsafe { policy.CurrentProfileTypes() }?;
    let rules = unsafe { policy.Rules() }?;
    let fits = |name: &str, profile: i32, action: NET_FW_ACTION| -> bool {
        let Ok(rule) = (unsafe { rules.Item(&BSTR::from(name)) }) else { return false };
        unsafe { rule_fits(&rule, program, port, profile, action) }.unwrap_or(false)
    };
    Ok(FirewallAnswer {
        private_allowed: fits(RULE_PRIVATE, NET_FW_PROFILE2_PRIVATE.0, NET_FW_ACTION_ALLOW),
        public_settled: fits(RULE_PUBLIC, NET_FW_PROFILE2_PUBLIC.0, NET_FW_ACTION_BLOCK),
        on_private_network: active & NET_FW_PROFILE2_PRIVATE.0 != 0,
        on_public_network: active & NET_FW_PROFILE2_PUBLIC.0 != 0,
    })
}

/// Whether a rule is switched on and says `action` for calls coming in to
/// `program` on TCP `port` from networks of `profile`.
unsafe fn rule_fits(rule: &INetFwRule, program: &Path, port: u16, profile: i32, action: NET_FW_ACTION) -> windows::core::Result<bool> {
    unsafe {
        let same_program = rule.ApplicationName()?.to_string().eq_ignore_ascii_case(&program.to_string_lossy());
        Ok(rule.Enabled()?.as_bool()
            && rule.Direction()? == NET_FW_RULE_DIR_IN
            && rule.Protocol()? == NET_FW_IP_PROTOCOL_TCP.0
            && rule.LocalPorts()? == port.to_string()
            && rule.Profiles()? & profile != 0
            && rule.Action()? == action
            && same_program)
    }
}
