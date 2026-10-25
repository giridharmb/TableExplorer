use crate::model::*;
use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::{
    components::{Route, Router, Routes},
    SsrMode,
    hooks::{use_params_map, use_query_map},
    params::ParamsMap,
    path,
};
use std::sync::atomic::{AtomicU32, Ordering};

pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <AutoReload options=options.clone()/>
                <HydrationScripts options/>
                <MetaTags/>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

// ---------- server functions (run on the server, called over HTTP from WASM) ----------

#[server]
pub async fn list_sources() -> Result<Vec<SourceInfo>, ServerFnError> {
    // shortcut: leptos 0.8.22's <Await> renders nothing during SSR if its future is ready on the
    // first poll (configured table lists need no I/O); yield once. Re-test and drop when upgrading leptos.
    tokio::task::yield_now().await;
    Ok(crate::server::source::infos().await)
}

#[server]
pub async fn get_columns(source: String, table: String) -> Result<Vec<Column>, ServerFnError> {
    use crate::server::source;
    let entry = source::get(&source).map_err(ServerFnError::new)?;
    if !entry.tables().await.map_err(ServerFnError::new)?.contains(&table) {
        return Err(ServerFnError::new(format!("unknown table {table:?}")));
    }
    entry.source().map_err(ServerFnError::new)?.columns(&table).await.map_err(ServerFnError::new)
}

#[server(input = server_fn::codec::Json)]
pub async fn run_search(req: SearchRequest) -> Result<SearchPage, ServerFnError> {
    crate::server::query::search(req).await.map_err(ServerFnError::new)
}

#[server(input = server_fn::codec::Json)]
pub async fn save_query(name: String, req: SearchRequest) -> Result<SavedQuery, ServerFnError> {
    // Only store searches that actually run (valid source, table, columns, operators).
    let check = SearchRequest { page: 1, page_size: 1, ..req.clone() };
    crate::server::query::search(check).await.map_err(ServerFnError::new)?;
    crate::server::store::save(&name, &req).await.map_err(ServerFnError::new)
}

#[server]
pub async fn load_query(id: String) -> Result<SavedQuery, ServerFnError> {
    crate::server::store::load(&id).await.map_err(ServerFnError::new)
}

#[server]
pub async fn recent_queries() -> Result<Vec<SavedQuery>, ServerFnError> {
    crate::server::store::recent(20).await.map_err(ServerFnError::new)
}

/// `?q=<json>` for a request; used by the shareable URL and the export links.
fn query_string(req: &SearchRequest) -> String {
    let q = serde_json::to_string(req).unwrap_or_default();
    [("q", q)].into_iter().collect::<ParamsMap>().to_query_string()
}

/// Rewrites the address bar without a router navigation (which would remount the page).
fn replace_url(url: &str) {
    if let Ok(h) = window().history() {
        let _ = h.replace_state_with_url(&leptos::wasm_bindgen::JsValue::NULL, "", Some(url));
    }
}

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen(inline_js = "export function copy_text(t) { navigator.clipboard?.writeText(t); }")]
extern "C" {
    fn copy_text(text: &str);
}
#[cfg(not(feature = "hydrate"))]
fn copy_text(_: &str) {}

fn err_text(e: &ServerFnError) -> String {
    match e {
        ServerFnError::ServerError(m) => m.clone(),
        other => other.to_string(),
    }
}

// ---------- reactive filter-builder state ----------

static NEXT_ID: AtomicU32 = AtomicU32::new(1);
fn next_id() -> u32 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy)]
struct RuleState {
    id: u32,
    column: RwSignal<String>,
    op: RwSignal<Op>,
    values: RwSignal<Vec<String>>,
    case_sensitive: RwSignal<bool>,
}

#[derive(Clone, Copy)]
struct GroupState {
    id: u32,
    join: RwSignal<Join>,
    negate: RwSignal<bool>,
    children: RwSignal<Vec<NodeState>>,
}

#[derive(Clone, Copy)]
enum NodeState {
    Rule(RuleState),
    Group(GroupState),
}

impl NodeState {
    fn id(&self) -> u32 {
        match self {
            NodeState::Rule(r) => r.id,
            NodeState::Group(g) => g.id,
        }
    }
    fn to_filter(&self) -> Filter {
        match self {
            NodeState::Rule(r) => Filter::Rule {
                column: r.column.get(),
                op: r.op.get(),
                values: r.values.get(),
                case_sensitive: r.case_sensitive.get(),
            },
            NodeState::Group(g) => g.to_filter(),
        }
    }
    fn from_filter(f: &Filter) -> Self {
        match f {
            Filter::Rule {
                column,
                op,
                values,
                case_sensitive,
            } => NodeState::Rule(RuleState {
                id: next_id(),
                column: RwSignal::new(column.clone()),
                op: RwSignal::new(*op),
                values: RwSignal::new(values.clone()),
                case_sensitive: RwSignal::new(*case_sensitive),
            }),
            Filter::Group {
                join,
                negate,
                children,
            } => NodeState::Group(GroupState {
                id: next_id(),
                join: RwSignal::new(*join),
                negate: RwSignal::new(*negate),
                children: RwSignal::new(children.iter().map(NodeState::from_filter).collect()),
            }),
        }
    }
}

impl GroupState {
    fn new() -> Self {
        GroupState {
            id: next_id(),
            join: RwSignal::new(Join::And),
            negate: RwSignal::new(false),
            children: RwSignal::new(vec![]),
        }
    }
    fn to_filter(&self) -> Filter {
        Filter::Group {
            join: self.join.get(),
            negate: self.negate.get(),
            children: self.children.get().iter().map(|c| c.to_filter()).collect(),
        }
    }
    fn contains(&self, id: u32) -> bool {
        self.children.with_untracked(|c| {
            c.iter()
                .any(|n| n.id() == id || matches!(n, NodeState::Group(g) if g.contains(id)))
        })
    }
}

type Columns = Signal<Vec<Column>>;

// ---------- drag & drop between groups ----------

#[derive(Clone, Copy)]
struct Drag {
    node: NodeState,
    from: GroupState,
}

fn can_move(d: &Drag, to: GroupState) -> bool {
    match d.node {
        // A group can't be dropped into itself or one of its own descendants.
        NodeState::Group(g) => g.id != to.id && !g.contains(to.id),
        NodeState::Rule(_) => true,
    }
}

/// Moves the dragged node into `to`, before the child `before` (or at the end).
fn move_node(d: Drag, to: GroupState, before: Option<u32>) {
    let id = d.node.id();
    if !can_move(&d, to) || before == Some(id) {
        return;
    }
    d.from.children.update(|c| c.retain(|n| n.id() != id));
    to.children.update(|c| {
        let i = before
            .and_then(|b| c.iter().position(|n| n.id() == b))
            .unwrap_or(c.len());
        c.insert(i, d.node);
    });
}

/// Drop target: (group id, child id to insert before; None = end of group).
type Target = (u32, Option<u32>);

#[derive(Clone, Copy)]
struct Dnd {
    dragging: RwSignal<Option<Drag>>,
    hover: RwSignal<Option<Target>>,
}

impl Dnd {
    fn over(&self, ev: &leptos::ev::DragEvent, to: GroupState, before: Option<u32>) {
        if !self
            .dragging
            .with_untracked(|d| d.as_ref().is_some_and(|d| can_move(d, to)))
        {
            return;
        }
        ev.prevent_default(); // marks this element as a valid drop target
        ev.stop_propagation();
        if self.hover.get_untracked() != Some((to.id, before)) {
            self.hover.set(Some((to.id, before)));
        }
    }
    fn drop(&self, ev: &leptos::ev::DragEvent, to: GroupState, before: Option<u32>) {
        ev.prevent_default();
        ev.stop_propagation();
        if let Some(d) = self.dragging.get_untracked() {
            move_node(d, to, before);
        }
        self.end();
    }
    fn end(&self) {
        self.dragging.set(None);
        self.hover.set(None);
    }
}

// ---------- components ----------

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    view! {
        <Stylesheet id="leptos" href="/pkg/webui.css"/>
        <Title text="Table Explorer · Rust WASM"/>
        <Router>
            <Routes fallback=|| view! { <div class="app"><div class="error-box">"Page not found."</div></div> }>
                // Async SSR: the server waits for every query, so the first HTML already holds the rows.
                <Route path=path!("") view=Home ssr=SsrMode::Async/>
                <Route path=path!("/s/:id") view=Shared ssr=SsrMode::Async/>
            </Routes>
        </Router>
    }
}

/// `/` and `/?q=<json>`: an ad-hoc search; with no `q`, the first available table.
/// `<Await>` (not `move || Suspend`) so the page is built exactly once, also on the server.
#[component]
fn Home() -> impl IntoView {
    let from_url = use_query_map()
        .with_untracked(|m| m.get("q"))
        .and_then(|q| serde_json::from_str::<SearchRequest>(&q).ok());
    view! {
        <Await future=list_sources() let:sources>
            {home_page(sources.clone().unwrap_or_default(), from_url)}
        </Await>
    }
}

fn home_page(sources: Vec<SourceInfo>, from_url: Option<SearchRequest>) -> AnyView {
    // Bad or stale JSON falls back to the first table; the server validates everything anyway.
    let init = from_url.or_else(|| {
        sources.iter().find(|s| s.error.is_none() && !s.tables.is_empty()).map(|s| SearchRequest::new(&s.id, &s.tables[0]))
    });
    match init {
        Some(init) => view! { <Explorer init sources saved=None/> }.into_any(),
        None => view! {
            <div class="app"><div class="error-box">
                <strong>"No data source available"</strong>
                <p>"Check config/app.toml and the server log."</p>
            </div></div>
        }.into_any(),
    }
}

/// `/s/<uuid>`: a saved search shared by someone, loaded from the storage database.
#[component]
fn Shared() -> impl IntoView {
    let id = use_params_map().with_untracked(|p| p.get("id").unwrap_or_default());
    let load = async move { Ok::<_, ServerFnError>((load_query(id).await?, list_sources().await?)) };
    view! {
        <Await future=load let:data>
            {match data.clone() {
                Ok((saved, sources)) => {
                    let init = saved.request.clone();
                    view! { <Explorer init sources saved=Some(saved)/> }.into_any()
                }
                Err(e) => view! {
                    <div class="app"><div class="error-box">
                        <strong>"Saved search unavailable"</strong>
                        <p>{err_text(&e)}</p>
                        <a href="/">"Start a new search"</a>
                    </div></div>
                }.into_any(),
            }}
        </Await>
    }
}

#[component]
fn Explorer(init: SearchRequest, sources: Vec<SourceInfo>, saved: Option<SavedQuery>) -> impl IntoView {
    let initial = init.clone();
    let source = RwSignal::new(init.source);
    let table = RwSignal::new(init.table);
    let root = match NodeState::from_filter(&init.filter) {
        NodeState::Group(g) => g,
        rule => {
            let g = GroupState::new();
            g.children.set(vec![rule]);
            g
        }
    };
    let quick = RwSignal::new(init.quick);
    let sort = RwSignal::new(init.sort);
    let page = RwSignal::new(init.page.max(1));
    let page_size = RwSignal::new(init.page_size);
    let show_sql = RwSignal::new(false);
    let (pending, set_pending) = signal(false);
    // Rules/groups are created in event handlers and can move between groups, so they are
    // owned here rather than by whichever group view happened to create them.
    provide_context(Owner::current().expect("Explorer runs inside an owner"));
    provide_context(Dnd {
        dragging: RwSignal::new(None),
        hover: RwSignal::new(None),
    });

    // Only needed for client-side rule editing, so skip it during SSR.
    let columns = LocalResource::new(move || get_columns(source.get(), table.get()));
    let cols: Columns =
        Signal::derive(move || columns.get().and_then(|r| r.ok()).unwrap_or_default());
    provide_context(cols);

    let filter = Memo::new(move |_| root.to_filter());
    // Anything that changes the result set restarts at page 1.
    Effect::watch(
        move || (filter.get(), quick.get(), source.get(), table.get(), page_size.get()),
        move |_, _, _| page.set(1),
        false,
    );

    let request = Memo::new(move |_| SearchRequest {
        source: source.get(),
        table: table.get(),
        filter: filter.get(),
        quick: quick.get(),
        sort: sort.get(),
        page: page.get(),
        page_size: page_size.get(),
    });
    let results = Resource::new(move || request.get(), run_search);

    // Keep the address bar shareable: once the search differs from what was loaded,
    // the URL becomes `/?q=<json>` (a saved `/s/<id>` link stays until you change something).
    Effect::new(move |_| {
        let req = request.get();
        if req != initial {
            replace_url(&format!("/{}", query_string(&req)));
        }
    });
    let export_href = move |fmt: &str| {
        let req = request.get();
        format!("/export/{fmt}{}", query_string(&SearchRequest { page: 1, ..req }))
    };
    let kind_of = {
        let sources = sources.clone();
        move |id: &str| sources.iter().find(|s| s.id == id).map(|s| s.kind.clone()).unwrap_or_default()
    };
    let kind = Memo::new(move |_| kind_of(&source.get()));

    let switch_table = move |src: String, t: String| {
        root.children.set(vec![]);
        root.negate.set(false);
        sort.set(None);
        quick.set(String::new());
        source.set(src);
        table.set(t);
    };

    // Save & share
    let save_open = RwSignal::new(false);
    let save_name = RwSignal::new(saved.as_ref().map(|s| s.name.clone()).unwrap_or_default());
    let save_status = RwSignal::new(None::<Result<String, String>>);
    let recent = LocalResource::new(recent_queries);
    let do_save = move || {
        save_status.set(None);
        leptos::task::spawn_local(async move {
            let result = save_query(save_name.get_untracked(), request.get_untracked()).await;
            save_status.set(Some(match result {
                Ok(sq) => {
                    recent.refetch();
                    let path = format!("/s/{}", sq.id);
                    replace_url(&path);
                    Ok(format!("{}{path}", window().location().origin().unwrap_or_default()))
                }
                Err(e) => Err(err_text(&e)),
            }));
        });
    };

    view! {
        <div class="app">
            <header class="topbar">
                <div class="brand">
                    <div class="logo">"⬡"</div>
                    <div>
                        <h1>"Table Explorer"</h1>
                        <p>"Rust → WebAssembly UI · server-side search, sort & pagination over PostgreSQL and BigQuery"</p>
                    </div>
                </div>
                <label class="table-pick">
                    <span class="kind" data-kind=move || kind.get()>{move || kind_label(&kind.get())}</span>
                    <select on:change=move |ev| {
                        if let Some((s, t)) = event_target_value(&ev).split_once('/') {
                            switch_table(s.to_string(), t.to_string());
                        }
                    }>
                        {sources.iter().map(|s| {
                            let group_label = s.label.clone();
                            let options = match &s.error {
                                Some(e) => view! { <option disabled>{format!("⚠ unavailable: {e}")}</option> }.into_any(),
                                None => s.tables.iter().map(|t| {
                                    let (id, t2) = (s.id.clone(), t.clone());
                                    view! {
                                        <option value=format!("{}/{}", s.id, t) prop:selected=move || source.get() == id && table.get() == t2>
                                            {t.clone()}
                                        </option>
                                    }
                                }).collect_view().into_any(),
                            };
                            view! { <optgroup label=group_label>{options}</optgroup> }
                        }).collect_view()}
                    </select>
                </label>
            </header>

            {saved.map(|sq| view! {
                <div class="banner">
                    <span>"📌 Saved search "<strong>{sq.name}</strong></span>
                    <span class="muted">{format!("created {} · id {}", sq.created_at, sq.id)}</span>
                </div>
            })}

            <div class="toolbar">
                <button class="primary" on:click=move |_| save_open.update(|o| *o = !*o)>"🔗 Save & share"</button>
                <a class="btn" href=move || export_href("xlsx") download>"⬇ Excel (.xlsx)"</a>
                <a class="btn" href=move || export_href("json") download>"⬇ JSON"</a>
                <span class="spacer"></span>
                <details class="saved-list">
                    <summary>"Saved searches"</summary>
                    <div class="saved-menu">
                            {move || match recent.get() {
                                None => view! { <div class="muted">"Loading…"</div> }.into_any(),
                                Some(r) => match r {
                                Ok(list) if list.is_empty() => view! { <div class="muted">"Nothing saved yet."</div> }.into_any(),
                                Ok(list) => list.into_iter().map(|q| view! {
                                    <a href=format!("/s/{}", q.id) rel="external">
                                        <strong>{q.name}</strong>
                                        <span class="muted">{format!("{} · {} · {}", q.request.source, q.request.table, q.created_at)}</span>
                                    </a>
                                }).collect_view().into_any(),
                                Err(e) => view! { <div class="muted">{err_text(&e)}</div> }.into_any(),
                            }}}
                    </div>
                </details>
            </div>

            <Show when=move || save_open.get()>
                <section class="card save-panel">
                    <div class="save-row">
                        <input
                            placeholder="Name this search, e.g. “VIP customers outside the US”"
                            bind:value=save_name
                            on:keydown=move |ev| if ev.key() == "Enter" { do_save() }
                        />
                        <button class="primary" on:click=move |_| do_save()>"Save"</button>
                    </div>
                    {move || save_status.get().map(|st| match st {
                        Ok(url) => {
                            let u = url.clone();
                            view! {
                                <div class="share">
                                    <span>"Anyone with this link sees this table with your filters:"</span>
                                    <div class="save-row">
                                        <input readonly prop:value=url on:focus=move |ev| { let _ = event_target::<leptos::web_sys::HtmlInputElement>(&ev).select(); }/>
                                        <button on:click=move |_| copy_text(&u)>"Copy"</button>
                                    </div>
                                </div>
                            }.into_any()
                        }
                        Err(e) => view! { <div class="error-box"><p>{e}</p></div> }.into_any(),
                    })}
                </section>
            </Show>

            <section class="card search">
                <div class="quick">
                    <span class="quick-icon">"⌕"</span>
                    <input
                        placeholder="Quick search across every column…"
                        prop:value=move || quick.get()
                        on:input=move |ev| quick.set(event_target_value(&ev))
                    />
                    <Show when=move || !quick.get().is_empty()>
                        <button class="icon" title="Clear" on:click=move |_| quick.set(String::new())>"✕"</button>
                    </Show>
                </div>
                <div class="builder-title">
                    <h2>"Advanced filters"</h2>
                    <div class="builder-actions">
                        <label class="toggle">
                            <input type="checkbox" bind:checked=show_sql/>
                            <span>"Show SQL"</span>
                        </label>
                        <button class="ghost" on:click=move |_| { root.children.set(vec![]); root.negate.set(false); }>
                            "Reset"
                        </button>
                    </div>
                </div>
                <GroupView group=root depth=0/>
            </section>

            <section class="card results" class:pending=move || pending.get()>
                <div class="progress"></div>
                <Transition fallback=|| view! { <div class="skeleton">"Loading rows…"</div> } set_pending>
                    {move || Suspend::new(async move {
                        match results.await {
                            Ok(p) => view! { <ResultView p sort page page_size show_sql quick/> }.into_any(),
                            Err(e) => view! {
                                <div class="error-box">
                                    <strong>"Query failed"</strong>
                                    <p>{err_text(&e)}</p>
                                    <small>"Adjust the filter — e.g. an invalid regex or a value that can’t be cast to the column type."</small>
                                </div>
                            }.into_any(),
                        }
                    })}
                </Transition>
            </section>
        </div>
    }
}

#[component]
fn GroupView(
    group: GroupState,
    depth: u32,
    #[prop(optional)] on_remove: Option<Callback<()>>,
) -> AnyView {
    let cols = expect_context::<Columns>();
    let owner = expect_context::<Owner>();
    let dnd = expect_context::<Dnd>();
    let add_rule = {
        let owner = owner.clone();
        move |_| {
            let first =
                cols.with_untracked(|c| c.first().map(|c| c.name.clone()).unwrap_or_default());
            let r = owner.with(|| RuleState {
                id: next_id(),
                column: RwSignal::new(first),
                op: RwSignal::new(Op::Contains),
                values: RwSignal::new(vec![]),
                case_sensitive: RwSignal::new(false),
            });
            group.children.update(|c| c.push(NodeState::Rule(r)));
        }
    };
    let add_group = move |_| {
        let g = owner.with(GroupState::new);
        group.children.update(|c| c.push(NodeState::Group(g)));
    };

    view! {
        <div class="group" class:nested={depth > 0} style=format!("--depth:{}", depth % 5)>
            <div class="group-head">
                <div class="seg">
                    <button class:on=move || group.join.get() == Join::And on:click=move |_| group.join.set(Join::And)>"AND"</button>
                    <button class:on=move || group.join.get() == Join::Or on:click=move |_| group.join.set(Join::Or)>"OR"</button>
                </div>
                <label class="toggle">
                    <input type="checkbox" bind:checked=group.negate/>
                    <span>"NOT"</span>
                </label>
                <span class="hint">
                    {move || if group.join.get() == Join::And { "all conditions must match" } else { "any condition may match" }}
                </span>
                <span class="spacer"></span>
                <button class="ghost" on:click=add_rule>"+ Rule"</button>
                <button class="ghost" on:click=add_group>"+ Group"</button>
                {on_remove.map(|cb| view! {
                    <button class="icon danger" title="Remove group" on:click=move |_| cb.run(())>"✕"</button>
                })}
            </div>
            <div
                class="children"
                class:drop-end=move || dnd.hover.get() == Some((group.id, None))
                on:dragover=move |ev| dnd.over(&ev, group, None)
                on:drop=move |ev| dnd.drop(&ev, group, None)
            >
                <For
                    each=move || group.children.get()
                    key=|n| n.id()
                    children=move |child| {
                        let id = child.id();
                        let remove = Callback::new(move |_: ()| group.children.update(|c| c.retain(|n| n.id() != id)));
                        let inner = match child {
                            NodeState::Rule(r) => view! { <RuleView rule=r on_remove=remove/> }.into_any(),
                            NodeState::Group(g) => view! { <GroupView group=g depth=depth + 1 on_remove=remove/> }.into_any(),
                        };
                        view! {
                            <div
                                class="item"
                                class:drop-before=move || dnd.hover.get() == Some((group.id, Some(id)))
                                class:dragging=move || dnd.dragging.with(|d| d.is_some_and(|d| d.node.id() == id))
                                on:dragover=move |ev| dnd.over(&ev, group, Some(id))
                                on:drop=move |ev| dnd.drop(&ev, group, Some(id))
                            >
                                <span
                                    class="handle"
                                    draggable="true"
                                    title="Drag to reorder or move into another group"
                                    on:dragstart=move |ev| {
                                        ev.stop_propagation();
                                        if let Some(dt) = ev.data_transfer() {
                                            dt.set_effect_allowed("move");
                                            let _ = dt.set_data("text/plain", "filter-node"); // Firefox needs data to start a drag
                                        }
                                        dnd.dragging.set(Some(Drag { node: child, from: group }));
                                    }
                                    on:dragend=move |_| dnd.end()
                                >"⠿"</span>
                                {inner}
                            </div>
                        }
                        .into_any()
                    }
                />
                <Show when=move || group.children.with(|c| c.is_empty())>
                    <div class="empty-hint">"No conditions yet — every row matches. Add a rule, or nest a group for (A AND B) OR C logic."</div>
                </Show>
            </div>
        </div>
    }
    .into_any()
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "postgres" => "PostgreSQL",
        "bigquery" => "BigQuery",
        _ => "Unknown",
    }
}

fn placeholder(op: Op) -> &'static str {
    match op {
        Op::Regex | Op::NotRegex => "regex, e.g. ^(alice|bob)\\d+@  ⏎",
        Op::Wildcard => "pattern, e.g. Order #10?? *DHL  ⏎",
        Op::Gt | Op::Gte | Op::Lt | Op::Lte => "value, e.g. 42 or 2023-01-01  ⏎",
        _ => "type a value, press Enter to add more  ⏎",
    }
}

#[component]
fn RuleView(rule: RuleState, on_remove: Callback<()>) -> impl IntoView {
    let cols = expect_context::<Columns>();
    let draft = RwSignal::new(String::new());
    let commit = move || {
        let v = draft.get_untracked();
        if !v.is_empty() {
            rule.values.update(|vs| {
                if !vs.contains(&v) {
                    vs.push(v)
                }
            });
            draft.set(String::new());
        }
    };

    view! {
        <div class="rule">
            <select class="col" on:change=move |ev| rule.column.set(event_target_value(&ev))>
                {move || cols.get().into_iter().map(|c| {
                    let n = c.name.clone();
                    view! {
                        <option value=c.name.clone() prop:selected=move || rule.column.get() == n>
                            {c.name.clone()}" · "{c.ty.clone()}
                        </option>
                    }
                }).collect_view()}
            </select>
            <select class="op" on:change=move |ev| if let Some(o) = Op::from_key(&event_target_value(&ev)) { rule.op.set(o) }>
                {Op::ALL.into_iter().map(|o| view! {
                    <option value=o.key() prop:selected=move || rule.op.get() == o>{o.label()}</option>
                }).collect_view()}
            </select>
            <Show
                when=move || rule.op.get().needs_value()
                fallback=|| view! { <div class="chips muted">"no value needed"</div> }
            >
                <div class="chips">
                    <For
                        each=move || rule.values.get()
                        key=|v| v.clone()
                        children=move |v| {
                            let v2 = v.clone();
                            view! {
                                <span class="chip">
                                    {v}
                                    <button title="Remove value" on:click=move |_| rule.values.update(|vs| vs.retain(|x| x != &v2))>"×"</button>
                                </span>
                            }
                        }
                    />
                    <input
                        placeholder=move || placeholder(rule.op.get())
                        bind:value=draft
                        on:keydown=move |ev| {
                            if ev.key() == "Enter" {
                                ev.prevent_default();
                                commit();
                            } else if ev.key() == "Backspace" && draft.get_untracked().is_empty() {
                                rule.values.update(|v| { v.pop(); });
                            }
                        }
                        on:blur=move |_| commit()
                    />
                </div>
                <button
                    class="case"
                    class:on=move || rule.case_sensitive.get()
                    title="Case sensitive"
                    on:click=move |_| rule.case_sensitive.update(|c| *c = !*c)
                >"Aa"</button>
            </Show>
            <button class="icon danger" title="Remove rule" on:click=move |_| on_remove.run(())>"✕"</button>
        </div>
    }
}

fn looks_broken(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.starts_with("#error")
        || l.starts_with("err_")
        || matches!(l.as_str(), "nan" | "infinity" | "-infinity" | "-1")
        || l.contains("\"error\"")
}

// shortcut: ASCII-only case folding for highlighting; non-ASCII matches still filter, just aren't marked.
fn highlight(s: &str, q: &str) -> AnyView {
    let q = q.trim().to_ascii_lowercase();
    let lower = s.to_ascii_lowercase();
    if q.is_empty() || !lower.contains(&q) {
        return s.to_string().into_any();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(pos) = lower[i..].find(&q) {
        let a = i + pos;
        out.push(s[i..a].to_string().into_any());
        out.push(view! { <mark>{s[a..a + q.len()].to_string()}</mark> }.into_any());
        i = a + q.len();
    }
    out.push(s[i..].to_string().into_any());
    out.into_any()
}

fn cell(v: Option<String>, ty: &str, q: &str) -> AnyView {
    match v {
        None => view! { <span class="null">"NULL"</span> }.into_any(),
        Some(s) if s.trim().is_empty() => view! { <span class="blank">"empty"</span> }.into_any(),
        Some(s) if ty == "boolean" => {
            let on = s == "true";
            view! { <span class="bool" class:on=on>{if on { "✓ true" } else { "✗ false" }}</span> }
                .into_any()
        }
        Some(s) if looks_broken(&s) => {
            view! { <span class="bad" title=s.clone()>"⚠ "{s.clone()}</span> }.into_any()
        }
        Some(s) => view! { <span class="val" title=s.clone()>{highlight(&s, q)}</span> }.into_any(),
    }
}

#[component]
fn ResultView(
    p: SearchPage,
    sort: RwSignal<Option<(String, bool)>>,
    page: RwSignal<u32>,
    page_size: RwSignal<u32>,
    show_sql: RwSignal<bool>,
    quick: RwSignal<String>,
) -> impl IntoView {
    let pages = ((p.total.max(0) as u64).div_ceil(p.page_size as u64)).max(1) as u32;
    let from = if p.total == 0 {
        0
    } else {
        (p.page - 1) as i64 * p.page_size as i64 + 1
    };
    let to = (from + p.rows.len() as i64 - 1).max(0);
    let q = quick.get_untracked();
    let types: Vec<String> = p.columns.iter().map(|c| c.ty.clone()).collect();
    let empty = p.rows.is_empty();
    let ncols = p.columns.len();

    let cycle_sort = move |name: String| {
        sort.update(|s| {
            *s = match s.take() {
                Some((c, false)) if c == name => Some((c, true)),
                Some((c, true)) if c == name => None,
                _ => Some((name, false)),
            }
        })
    };

    view! {
        <div class="stats">
            <span class="pill strong">{format!("{} matching rows", p.total)}</span>
            <span class="pill">{format!("showing {from}–{to}")}</span>
            <span class="pill">{format!("{ncols} columns")}</span>
            <span class="pill">{format!("{:.1} ms server time", p.elapsed_ms)}</span>
        </div>
        <Show when=move || show_sql.get()>
            <pre class="sql">{p.sql.clone()}</pre>
        </Show>
        <div class="table-wrap">
            <table>
                <thead>
                    <tr>
                        <th class="rownum">"#"</th>
                        {p.columns.iter().map(|c| {
                            let name = c.name.clone();
                            let n1 = name.clone();
                            let n2 = name.clone();
                            view! {
                                <th on:click=move |_| cycle_sort(n1.clone()) title="Click to sort (server side)">
                                    <div class="th">
                                        <span class="th-name">{name}</span>
                                        <span class="sort">
                                            {move || match sort.get() {
                                                Some((s, false)) if s == n2 => "▲",
                                                Some((s, true)) if s == n2 => "▼",
                                                _ => "↕",
                                            }}
                                        </span>
                                    </div>
                                    <span class="ty">{c.ty.clone()}</span>
                                </th>
                            }
                        }).collect_view()}
                    </tr>
                </thead>
                <tbody>
                    {p.rows.into_iter().enumerate().map(|(i, row)| view! {
                        <tr>
                            <td class="rownum">{from + i as i64}</td>
                            {row.into_iter().zip(types.iter()).map(|(v, ty)| view! { <td>{cell(v, ty, &q)}</td> }).collect_view()}
                        </tr>
                    }).collect_view()}
                </tbody>
            </table>
            {empty.then(|| view! { <div class="no-rows">"No rows match these filters."</div> })}
        </div>
        <nav class="pager">
            <label class="page-size">
                "Rows per page"
                <select on:change=move |ev| page_size.set(event_target_value(&ev).parse().unwrap_or(25))>
                    {[10u32, 25, 50, 100, 200].into_iter().map(|n| view! {
                        <option value=n.to_string() prop:selected=move || page_size.get() == n>{n}</option>
                    }).collect_view()}
                </select>
            </label>
            <span class="spacer"></span>
            <button disabled={p.page <= 1} on:click=move |_| page.set(1)>"«"</button>
            <button disabled={p.page <= 1} on:click=move |_| page.update(|n| *n = n.saturating_sub(1).max(1))>"‹ Prev"</button>
            <span class="page-of">
                "Page "
                <input
                    type="number"
                    min="1"
                    max=pages
                    prop:value=p.page
                    on:change=move |ev| {
                        let n: u32 = event_target_value(&ev).parse().unwrap_or(1);
                        page.set(n.clamp(1, pages));
                    }
                />
                {format!(" of {pages}")}
            </span>
            <button disabled={p.page >= pages} on:click=move |_| page.update(|n| *n = (*n + 1).min(pages))>"Next ›"</button>
            <button disabled={p.page >= pages} on:click=move |_| page.set(pages)>"»"</button>
        </nav>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(col: &str) -> Filter {
        Filter::Rule {
            column: col.into(),
            op: Op::Contains,
            values: vec!["x".into()],
            case_sensitive: false,
        }
    }
    fn group(join: Join, children: Vec<Filter>) -> Filter {
        Filter::Group {
            join,
            negate: false,
            children,
        }
    }
    fn as_group(n: NodeState) -> GroupState {
        match n {
            NodeState::Group(g) => g,
            NodeState::Rule(_) => panic!("expected group"),
        }
    }

    #[test]
    fn url_json_round_trips_nested_filter() {
        Owner::new().with(|| {
            let f = group(
                Join::And,
                vec![
                    rule("a"),
                    group(Join::Or, vec![rule("b"), group(Join::And, vec![rule("c")])]),
                ],
            );
            let req = SearchRequest {
                source: "s".into(),
                table: "t".into(),
                filter: f.clone(),
                quick: "q".into(),
                sort: Some(("a".into(), true)),
                page: 3,
                page_size: 50,
            };
            let back: SearchRequest =
                serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
            assert_eq!(back, req);
            assert_eq!(NodeState::from_filter(&f).to_filter(), f);
        })
    }

    #[test]
    fn drag_moves_nodes_but_never_into_own_subtree() {
        Owner::new().with(|| {
            let root = as_group(NodeState::from_filter(&group(
                Join::And,
                vec![rule("a"), group(Join::Or, vec![rule("b")])],
            )));
            let [a, inner] = root.children.get_untracked()[..] else {
                panic!()
            };
            let inner = as_group(inner);

            // rule "a" → before "b" inside the OR group
            let b_id = inner.children.get_untracked()[0].id();
            move_node(
                Drag {
                    node: a,
                    from: root,
                },
                inner,
                Some(b_id),
            );
            assert_eq!(
                root.to_filter(),
                group(Join::And, vec![group(Join::Or, vec![rule("a"), rule("b")])])
            );

            // the OR group into itself → no-op
            let before = root.to_filter();
            move_node(
                Drag {
                    node: NodeState::Group(inner),
                    from: root,
                },
                inner,
                None,
            );
            assert_eq!(root.to_filter(), before);
        })
    }
}
