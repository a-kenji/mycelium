#![allow(non_snake_case)]

mod api;

use dioxus::prelude::*;
use dioxus_free_icons::icons::fa_solid_icons::FaChevronLeft;
use dioxus_free_icons::Icon;
use mycelium::peer_manager::PeerType;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::{cmp::Ordering, str::FromStr};
use tracing::Level;

const STYLES_CSS: &str = manganis::mg!(file("assets/styles.css"));

const DEFAULT_SERVER_ADDR: std::net::SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8989);

#[derive(Clone, Routable, Debug, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[layout(Layout)]
        #[route("/")]
        Home,
        #[route("/peers")]
        Peers,
        #[route("/routes")]
        Routes,
    #[end_layout]
    #[route("/:..route")]
    PageNotFound { route: Vec<String> },
}

#[derive(Clone, Debug)]
struct ServerState {
    address: String,
    connected: bool,
}

fn main() {
    // Init logger
    dioxus_logger::init(Level::INFO).expect("failed to init logger");
    dioxus::launch(App);
}

#[component]
fn Layout() -> Element {
    let sidebar_collapsed = use_signal(|| false);

    rsx! {
        div { class: "app-container",
            Header {}
            div { class: "content-container",
                Sidebar { collapsed: sidebar_collapsed }
                main { class: if *sidebar_collapsed.read() { "main-content expanded" } else { "main-content" },
                    Outlet::<Route> {}
                }
            }
        }
    }
}

#[component]
fn App() -> Element {
    // Shared state components
    use_context_provider(|| {
        Signal::new(ServerState {
            address: DEFAULT_SERVER_ADDR.to_string(),
            connected: false,
        })
    });

    rsx! {
        Router::<Route> {}
    }
}

#[component]
fn Header() -> Element {
    let fetched_node_info = use_resource(move || api::get_node_info(DEFAULT_SERVER_ADDR));
    rsx! {
        header {
            h1 { "Mycelium Network Dashboard" }
            div { class: "node-info",
                { match &*fetched_node_info.read_unchecked() {
                    Some(Ok(info)) => rsx! {
                        span { "Subnet: {info.node_subnet}" }
                        span { class: "separator", "|" }
                        span { "Public Key: {info.node_pubkey}" }
                    },
                    Some(Err(_)) => rsx! { span { "Error loading node info" } },
                    None => rsx! { span { "Loading node info..." } },
                }}
            }
        }
    }
}

#[component]
fn Sidebar(collapsed: Signal<bool>) -> Element {
    rsx! {
        nav { class: if *collapsed.read() { "sidebar collapsed" } else { "sidebar" },
            ul {
                li { Link { to: Route::Home {}, "Home" } }
                li { Link { to: Route::Peers {}, "Peers" } }
                li { Link { to: Route::Routes {}, "Routes" } }
            }
        }
        button { class: if *collapsed.read() { "toggle-sidebar collapsed" } else { "toggle-sidebar" },
            onclick: {
                let c = *collapsed.read();
                move |_| collapsed.set(!c)
            },
            Icon {
                icon: FaChevronLeft,
            }
        }
    }
}

#[component]
fn Home() -> Element {
    let mut server_state = use_context::<Signal<ServerState>>();
    let mut new_address = use_signal(|| DEFAULT_SERVER_ADDR.to_string());
    let mut error_message = use_signal(|| None::<String>);

    let fetched_node_info = use_resource(move || {
        let address = server_state.read().address.clone();
        async move {
            match SocketAddr::from_str(&address) {
                Ok(addr) => {
                    let result = api::get_node_info(addr).await;
                    match result {
                        Ok(info) => {
                            server_state.write().connected = true;
                            Ok(info)
                        }
                        Err(e) => {
                            server_state.write().connected = false;
                            Err(AppError::from(e))
                        }
                    }
                }
                Err(_) => Err(AppError::AddressError("Invalid address format".into())),
            }
        }
    });

    let connect = move |_| {
        let new_addr = new_address().clone().to_string();
        if SocketAddr::from_str(&new_addr).is_ok() {
            server_state.write().address = new_addr;
            error_message.set(None);
            // fetched_node_info.refetch();
        } else {
            error_message.set(Some("Invalid address format".to_string()));
        }
    };

    rsx! {
        div { class: "home-container",
            h2 { "Node information" }
            div { class: "server-input",
                input {
                    placeholder: "Server address (e.g. 127.0.0.1:8989)",
                    value: "{new_address}",
                    oninput: move |event| new_address.set(event.value().clone())
                }
                button { onclick: connect, "Connect" }
            }
            if let Some(err_msg) = error_message.read().as_ref() {
                 { rsx! { p { class: "error", "{err_msg}" } } }
            }
            if !server_state.read().connected {
                { rsx! { p { class: "warning", "Server is not responding. Please check the server address and try again." } } }
            }
            match fetched_node_info.read().as_ref() {
                Some(Ok(info)) => {
                { rsx! {
                    p {
                        "Node subnet: ",
                        span { class: "bold", "{info.node_subnet}" }
                    }
                    p {
                        "Node public key: ",
                        span { class: "bold", "{info.node_pubkey }" }
                    }
                } }
                },
                Some(Err(e)) => { rsx! { p { class: "error", "Error: {e}" } } },
                None => { rsx! { p { "Loading..." } } },
            }
        }
    }
}

#[component]
fn Peers() -> Element {
    let fetched_peers = use_resource(move || api::get_peers(DEFAULT_SERVER_ADDR));
    match &*fetched_peers.read_unchecked() {
        Some(Ok(peers)) => rsx! { {PeersTable(peers.clone()) } },
        Some(Err(e)) => rsx! { div { "An error has occurred while fetching the peers: {e}" } },
        None => rsx! { div { "Loading peers..." } },
    }
}

#[component]
fn Routes() -> Element {
    rsx! {
        SelectedRoutesTable {}
        FallbackRoutesTable {}
    }
}

#[component]
fn PageNotFound(route: Vec<String>) -> Element {
    rsx! {
        p { "Page not found"}
    }
}

pub struct PeerTypeWrapper(pub mycelium::peer_manager::PeerType);
impl Ord for PeerTypeWrapper {
    fn cmp(&self, other: &Self) -> Ordering {
        match (&self.0, &other.0) {
            (PeerType::Static, PeerType::Static) => Ordering::Equal,
            (PeerType::Static, _) => Ordering::Less,
            (PeerType::LinkLocalDiscovery, PeerType::Static) => Ordering::Greater,
            (PeerType::LinkLocalDiscovery, PeerType::LinkLocalDiscovery) => Ordering::Equal,
            (PeerType::LinkLocalDiscovery, PeerType::Inbound) => Ordering::Less,
            (PeerType::Inbound, PeerType::Inbound) => Ordering::Equal,
            (PeerType::Inbound, _) => Ordering::Greater,
        }
    }
}

impl PartialOrd for PeerTypeWrapper {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for PeerTypeWrapper {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for PeerTypeWrapper {}

#[derive(Clone)]
enum SortDirection {
    Ascending,
    Descending,
}

fn sort_peers(
    peers: &mut [mycelium::peer_manager::PeerStats],
    column: &str,
    direction: &SortDirection,
) {
    peers.sort_by(|a, b| {
        let cmp = match column {
            "Endpoint" => a.endpoint.cmp(&b.endpoint),
            "Type" => PeerTypeWrapper(a.pt.clone()).cmp(&PeerTypeWrapper(b.pt.clone())),
            "Connection State" => a.connection_state.cmp(&b.connection_state),
            "Tx bytes" => a.tx_bytes.cmp(&b.tx_bytes),
            "Rx bytes" => a.rx_bytes.cmp(&b.rx_bytes),
            _ => Ordering::Equal,
        };
        match direction {
            SortDirection::Ascending => cmp,
            SortDirection::Descending => cmp.reverse(),
        }
    });
}

fn PeersTable(peers: Vec<mycelium::peer_manager::PeerStats>) -> Element {
    let mut current_page = use_signal(|| 0);
    let items_per_page = 20;
    let mut sort_column = use_signal(|| "Type".to_string());
    let mut sort_direction = use_signal(|| SortDirection::Ascending);
    let peers_len = peers.len();

    let mut change_page = move |delta: i32| {
        let cur_page = *current_page.read() as i32;
        current_page.set(
            (cur_page + delta)
                .max(0)
                .min((peers_len - 1) as i32 / items_per_page as i32) as usize,
        );
    };

    let mut sort_peers_signal = move |column: String| {
        if column == *sort_column.read() {
            let new_sort_direction = match *sort_direction.read() {
                SortDirection::Ascending => SortDirection::Descending,
                SortDirection::Descending => SortDirection::Ascending,
            };
            sort_direction.set(new_sort_direction);
        } else {
            sort_column.set(column);
            sort_direction.set(SortDirection::Descending);
        }
    };

    let sorted_peers = use_memo(move || {
        let mut sorted = peers.clone();
        sort_peers(&mut sorted, &sort_column.read(), &sort_direction.read());
        sorted
    });

    let start = current_page * items_per_page;
    let end = (start + items_per_page).min(peers_len);
    let current_peers = &sorted_peers.read()[start..end];

    rsx! {
        div { class: "peers-table",
            h2 { "Peers" }
            div { class: "table-container",
                table {
                    thead {
                        tr {
                            th { class: "endpoint-column",
                                onclick: move |_| sort_peers_signal("Endpoint".to_string()),
                                "Endpoint {get_sort_indicator(sort_column, sort_direction, \"Endpoint\".to_string())}"
                            }
                            th { class: "type-column",
                                onclick: move |_| sort_peers_signal("Type".to_string()),
                                "Type {get_sort_indicator(sort_column, sort_direction, \"Type\".to_string())}"
                            }
                            th { class: "connection-state-column",
                                onclick: move |_| sort_peers_signal("Connection State".to_string()),
                                "Connection State {get_sort_indicator(sort_column, sort_direction, \"Connection State\".to_string())}"
                            }
                            th { class: "tx-bytes-column",
                                onclick: move |_| sort_peers_signal("Tx bytes".to_string()),
                                "Tx bytes {get_sort_indicator(sort_column, sort_direction, \"Tx bytes\".to_string())}"
                            }
                            th { class: "rx-bytes-column",
                                onclick: move |_| sort_peers_signal("Rx bytes".to_string()),
                                "Rx bytes {get_sort_indicator(sort_column, sort_direction, \"Rx bytes\".to_string())}"
                            }
                        }
                    }
                    tbody {
                        for peer in current_peers {
                            tr {
                                td { class: "endpoint-column", "{peer.endpoint}" }
                                td { class: "type-column", "{peer.pt}" }
                                td { class: "connection-state-column", "{peer.connection_state}" }
                                td { class: "tx-bytes-column", "{peer.tx_bytes}" }
                                td { class: "rx-bytes-column", "{peer.rx_bytes}" }
                            }
                        }
                    }
                }
            }
            div { class: "pagination",
                button {
                    disabled: *current_page.read() == 0,
                    onclick: move |_| change_page(-1),
                    "Previous"
                }
                span { "Page {current_page + 1}" }
                button {
                    disabled: (current_page + 1) * items_per_page >= peers_len,
                    onclick: move |_| change_page(1),
                    "Next"
                }
            }
        }
    }
}

#[component]
fn SelectedRoutesTable() -> Element {
    let fetched_selected_routes =
        use_resource(move || api::get_selected_routes(DEFAULT_SERVER_ADDR));

    match &*fetched_selected_routes.read_unchecked() {
        Some(Ok(routes)) => {
            rsx! { { RoutesTable(routes.clone(), "Selected".to_string())  } }
        }
        Some(Err(e)) => rsx! { div { "An error has occurred while fetching selected routes: {e}" }},
        None => rsx! { div { "Loading selected routes..." }},
    }
}

#[component]
fn FallbackRoutesTable() -> Element {
    let fetched_fallback_routes =
        use_resource(move || api::get_fallback_routes(DEFAULT_SERVER_ADDR));

    match &*fetched_fallback_routes.read_unchecked() {
        Some(Ok(routes)) => {
            rsx! { { RoutesTable(routes.clone(), "Fallback".to_string()) } }
        }
        Some(Err(e)) => rsx! { div { "An error has occurred while fetching fallback routes: {e}" }},
        None => rsx! { div { "Loading fallback routes..." }},
    }
}

fn get_sort_indicator(
    sort_column: Signal<String>,
    sort_direction: Signal<SortDirection>,
    column: String,
) -> String {
    if *sort_column.read() == column {
        match *sort_direction.read() {
            SortDirection::Ascending => " ↑".to_string(),
            SortDirection::Descending => " ↓".to_string(),
        }
    } else {
        "".to_string()
    }
}

fn sort_routes(routes: &mut [mycelium_api::Route], column: &str, direction: &SortDirection) {
    routes.sort_by(|a, b| {
        let cmp = match column {
            "Subnet" => a.subnet.cmp(&b.subnet),
            "Next-hop" => a.next_hop.cmp(&b.next_hop),
            "Metric" => a.metric.cmp(&b.metric),
            "Seqno" => a.seqno.cmp(&b.seqno),
            _ => Ordering::Equal,
        };
        match direction {
            SortDirection::Ascending => cmp,
            SortDirection::Descending => cmp.reverse(),
        }
    });
}

fn RoutesTable(routes: Vec<mycelium_api::Route>, table_name: String) -> Element {
    let mut current_page = use_signal(|| 0);
    let items_per_page = 10;
    let mut sort_column = use_signal(|| "Subnet".to_string());
    let mut sort_direction = use_signal(|| SortDirection::Descending);
    let routes_len = routes.len();

    let mut change_page = move |delta: i32| {
        let cur_page = *current_page.read() as i32;
        current_page.set(
            (cur_page + delta)
                .max(0)
                .min((routes_len - 1) as i32 / items_per_page as i32) as usize,
        );
    };

    let mut sort_routes_signal = move |column: String| {
        if column == *sort_column.read() {
            let new_sort_direction = match *sort_direction.read() {
                SortDirection::Ascending => SortDirection::Descending,
                SortDirection::Descending => SortDirection::Ascending,
            };
            sort_direction.set(new_sort_direction);
        } else {
            sort_column.set(column);
            sort_direction.set(SortDirection::Ascending);
        }
    };

    let sorted_routes = use_memo(move || {
        let mut sorted = routes.clone();
        sort_routes(&mut sorted, &sort_column.read(), &sort_direction.read());
        sorted
    });

    let start = current_page * items_per_page;
    let end = (start + items_per_page).min(routes_len);
    let current_routes = &sorted_routes.read()[start..end];

    rsx! {
        div { class: "{table_name.to_lowercase()}-routes",
            h2 { "{table_name} Routes" }
            div { class: "table-container",
                table {
                    thead {
                        tr {
                            th { class: "subnet-column",
                                onclick: move |_| sort_routes_signal("Subnet".to_string()),
                                "Subnet {get_sort_indicator(sort_column, sort_direction, \"Subnet\".to_string())}"
                            }
                            th { class: "next-hop-column",
                                onclick: move |_| sort_routes_signal("Next-hop".to_string()),
                                "Next-hop {get_sort_indicator(sort_column, sort_direction, \"Next-hop\".to_string())}"
                            }
                            th { class: "metric-column",
                                onclick: move |_| sort_routes_signal("Metric".to_string()),
                                "Metric {get_sort_indicator(sort_column, sort_direction, \"Metric\".to_string())}"
                            }
                            th { class: "seqno_column",
                                onclick: move |_| sort_routes_signal("Seqno".to_string()),
                                "Seqno {get_sort_indicator(sort_column, sort_direction, \"Seqno\".to_string())}"
                            }
                        }
                    }
                    tbody {
                        for route in current_routes {
                            tr {
                                td { class: "subnet-column", "{route.subnet}" }
                                td { class: "next-hop-column", "{route.next_hop}" }
                                td { class: "metric-column", "{route.metric}" }
                                td { class: "seqno-column", "{route.seqno}" }
                            }
                        }
                    }
                }
            }
            div { class: "pagination",
                button {
                    disabled: *current_page.read() == 0,
                    onclick: move |_| change_page(-1),
                    "Previous"
                }
                span { "Page {current_page + 1}" }
                button {
                    disabled: (current_page + 1) * items_per_page >= routes_len,
                    onclick: move |_| change_page(1),
                    "Next"
                }
            }
        }
    }
}

enum AppError {
    NetworkError(reqwest::Error), // reqwest errors
    AddressError(String),         // address parsing errors
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::NetworkError(e) => write!(f, "Network error: {}", e),
            AppError::AddressError(e) => write!(f, "Address error: {}", e),
        }
    }
}

impl From<reqwest::Error> for AppError {
    fn from(value: reqwest::Error) -> Self {
        AppError::NetworkError(value)
    }
}
