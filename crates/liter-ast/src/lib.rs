//! Abstract Syntax Tree node types for Liter-rs.
//!
//! Mirrors the AST built by SQLite's Lemon-generated parser. All statement
//! and expression variants are defined here.
//!
//! ## Status
//! Phase 3 — stub type definitions only.

/// A complete SQL statement.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Select(Box<SelectStmt>),
    Insert(Box<InsertStmt>),
    Update(Box<UpdateStmt>),
    Delete(Box<DeleteStmt>),
    Create(Box<CreateStmt>),
    Drop(Box<DropStmt>),
    Alter(Box<AlterStmt>),
    Begin(TransactionKind),
    Commit,
    Rollback { savepoint: Option<String> },
    Savepoint(String),
    Release(String),
    Attach { expr: Expr, name: String, key: Option<Expr> },
    Detach(String),
    Pragma { schema: Option<String>, name: String, value: Option<PragmaValue> },
    Vacuum { schema: Option<String>, into: Option<Expr> },
    Reindex { target: Option<String> },
    Analyze { target: Option<String> },
    Explain { query_plan: bool, stmt: Box<Stmt> },
}

/// SELECT statement.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectStmt {
    pub with: Option<WithClause>,
    pub body: SelectBody,
    pub order_by: Vec<OrderingTerm>,
    pub limit: Option<LimitClause>,
}

/// The core of a SELECT (may be a compound via UNION/INTERSECT/EXCEPT).
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum SelectBody {
    Simple(SimpleSelect),
    Compound {
        op: CompoundOp,
        left: Box<SelectBody>,
        right: Box<SelectBody>,
    },
    Values(Vec<Vec<Expr>>),
}

/// Simple (non-compound) SELECT.
#[derive(Debug, Clone, PartialEq)]
pub struct SimpleSelect {
    pub distinct: DistinctKind,
    pub result_columns: Vec<ResultColumn>,
    pub from: Option<FromClause>,
    pub where_: Option<Expr>,
    pub group_by: Vec<Expr>,
    pub having: Option<Expr>,
    pub window: Vec<WindowDef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistinctKind { All, Distinct }

#[derive(Debug, Clone, PartialEq)]
pub enum ResultColumn {
    Star,
    TableStar(String),
    Expr { expr: Expr, alias: Option<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct FromClause {
    pub tables: Vec<TableOrSubquery>,
    pub joins: Vec<JoinClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TableOrSubquery {
    Table { schema: Option<String>, name: String, alias: Option<String>, indexed: IndexedKind },
    Subquery { select: Box<SelectStmt>, alias: Option<String> },
    TableFunction { schema: Option<String>, name: String, args: Vec<Expr>, alias: Option<String> },
    Joined(Box<JoinClause>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum IndexedKind {
    None,
    NotIndexed,
    IndexedBy(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct JoinClause {
    pub op: JoinOp,
    pub table: Box<TableOrSubquery>,
    pub constraint: Option<JoinConstraint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinOp {
    Inner,
    Left,
    Right,
    Full,
    Cross,
    NaturalInner,
    NaturalLeft,
    NaturalRight,
    NaturalFull,
    NaturalCross,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JoinConstraint {
    On(Expr),
    Using(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderingTerm {
    pub expr: Expr,
    pub direction: SortDirection,
    pub nulls: NullsOrder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection { Asc, Desc }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NullsOrder { First, Last, Default }

#[derive(Debug, Clone, PartialEq)]
pub struct LimitClause {
    pub limit: Expr,
    pub offset: Option<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompoundOp { Union, UnionAll, Intersect, Except }

/// INSERT statement.
#[derive(Debug, Clone, PartialEq)]
pub struct InsertStmt {
    pub with: Option<WithClause>,
    pub or: Option<ConflictAction>,
    pub schema: Option<String>,
    pub table: String,
    pub alias: Option<String>,
    pub columns: Vec<String>,
    pub source: InsertSource,
    pub returning: Vec<ResultColumn>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InsertSource {
    Values(Vec<Vec<Expr>>),
    Select(Box<SelectStmt>),
    DefaultValues,
}

/// UPDATE statement.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateStmt {
    pub with: Option<WithClause>,
    pub or: Option<ConflictAction>,
    pub table: QualifiedTable,
    pub assignments: Vec<Assignment>,
    pub from: Option<FromClause>,
    pub where_: Option<Expr>,
    pub returning: Vec<ResultColumn>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub columns: Vec<String>,
    pub value: Expr,
}

/// DELETE statement.
#[derive(Debug, Clone, PartialEq)]
pub struct DeleteStmt {
    pub with: Option<WithClause>,
    pub table: QualifiedTable,
    pub where_: Option<Expr>,
    pub returning: Vec<ResultColumn>,
}

/// CREATE statement variants.
#[derive(Debug, Clone, PartialEq)]
pub enum CreateStmt {
    Table(CreateTable),
    Index(CreateIndex),
    View(CreateView),
    Trigger(CreateTrigger),
    VirtualTable(CreateVirtualTable),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateTable {
    pub temp: bool,
    pub if_not_exists: bool,
    pub schema: Option<String>,
    pub name: String,
    pub body: CreateTableBody,
    pub options: TableOptions,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CreateTableBody {
    Columns { columns: Vec<ColumnDef>, constraints: Vec<TableConstraint> },
    As(Box<SelectStmt>),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableOptions {
    pub without_rowid: bool,
    pub strict: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDef {
    pub name: String,
    pub type_name: Option<TypeName>,
    pub constraints: Vec<ColumnConstraint>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeName {
    pub name: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnConstraint {
    PrimaryKey { direction: Option<SortDirection>, conflict: Option<ConflictAction>, autoincrement: bool },
    NotNull { conflict: Option<ConflictAction> },
    Unique { conflict: Option<ConflictAction> },
    Check(Expr),
    Default(DefaultValue),
    Collate(String),
    References(ForeignKeyClause),
    Generated { expr: Expr, stored: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub enum DefaultValue {
    Expr(Expr),
    LiteralValue(LiteralValue),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableConstraint {
    pub name: Option<String>,
    pub kind: TableConstraintKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TableConstraintKind {
    PrimaryKey { columns: Vec<IndexedColumn>, conflict: Option<ConflictAction> },
    Unique { columns: Vec<IndexedColumn>, conflict: Option<ConflictAction> },
    Check(Expr),
    ForeignKey { columns: Vec<String>, clause: ForeignKeyClause },
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexedColumn {
    pub expr: Expr,
    pub collate: Option<String>,
    pub direction: Option<SortDirection>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForeignKeyClause {
    pub table: String,
    pub columns: Vec<String>,
    pub actions: Vec<ForeignKeyAction>,
    pub deferrable: Option<DeferrableKind>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ForeignKeyAction {
    OnDelete(ReferentialAction),
    OnUpdate(ReferentialAction),
    Match(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferentialAction { SetNull, SetDefault, Cascade, Restrict, NoAction }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferrableKind { Deferrable, NotDeferrable, InitiallyDeferred, InitiallyImmediate }

#[derive(Debug, Clone, PartialEq)]
pub struct CreateIndex {
    pub unique: bool,
    pub if_not_exists: bool,
    pub schema: Option<String>,
    pub name: String,
    pub table: String,
    pub columns: Vec<IndexedColumn>,
    pub where_: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateView {
    pub temp: bool,
    pub if_not_exists: bool,
    pub schema: Option<String>,
    pub name: String,
    pub columns: Vec<String>,
    pub select: Box<SelectStmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateTrigger {
    pub temp: bool,
    pub if_not_exists: bool,
    pub schema: Option<String>,
    pub name: String,
    pub time: TriggerTime,
    pub event: TriggerEvent,
    pub table: String,
    pub for_each_row: bool,
    pub when: Option<Expr>,
    pub body: Vec<TriggerStmt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerTime { Before, After, InsteadOf }

#[derive(Debug, Clone, PartialEq)]
pub enum TriggerEvent {
    Delete,
    Insert,
    Update(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum TriggerStmt {
    Update(UpdateStmt),
    Insert(InsertStmt),
    Delete(DeleteStmt),
    Select(SelectStmt),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateVirtualTable {
    pub if_not_exists: bool,
    pub schema: Option<String>,
    pub name: String,
    pub module: String,
    pub args: Vec<String>,
}

/// DROP statement.
#[derive(Debug, Clone, PartialEq)]
pub struct DropStmt {
    pub kind: DropKind,
    pub if_exists: bool,
    pub schema: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropKind { Table, Index, View, Trigger }

/// ALTER TABLE statement.
#[derive(Debug, Clone, PartialEq)]
pub struct AlterStmt {
    pub schema: Option<String>,
    pub table: String,
    pub action: AlterAction,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AlterAction {
    RenameTo(String),
    RenameColumn { from: String, to: String },
    AddColumn(ColumnDef),
    DropColumn(String),
}

/// SQL expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Literal(LiteralValue),
    Bind(BindParam),
    Column { schema: Option<String>, table: Option<String>, name: String },
    Unary { op: UnaryOp, operand: Box<Expr> },
    Binary { op: BinaryOp, left: Box<Expr>, right: Box<Expr> },
    Function { schema: Option<String>, name: String, args: FunctionArgs, filter: Option<Box<Expr>>, over: Option<WindowSpec> },
    Cast { expr: Box<Expr>, type_name: TypeName },
    Collate { expr: Box<Expr>, collation: String },
    Like { not: bool, op: LikeOp, lhs: Box<Expr>, rhs: Box<Expr>, escape: Option<Box<Expr>> },
    IsNull { not: bool, expr: Box<Expr> },
    Is { not: bool, lhs: Box<Expr>, rhs: Box<Expr> },
    Between { not: bool, expr: Box<Expr>, low: Box<Expr>, high: Box<Expr> },
    In { not: bool, expr: Box<Expr>, rhs: InRhs },
    Exists { not: bool, select: Box<SelectStmt> },
    Case { base: Option<Box<Expr>>, arms: Vec<CaseArm>, else_: Option<Box<Expr>> },
    RowValue(Vec<Expr>),
    Subquery(Box<SelectStmt>),
    Raise { kind: RaiseKind, message: Option<Box<Expr>> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiteralValue {
    Null,
    True,
    False,
    Integer(i64),
    Float(f64),
    Text(String),
    Blob(Vec<u8>),
    CurrentDate,
    CurrentTime,
    CurrentTimestamp,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BindParam {
    Positional,
    Numbered(u32),
    Named(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp { Minus, Plus, BitNot, Not }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add, Sub, Mul, Div, Mod,
    BitAnd, BitOr, LShift, RShift,
    Concat,
    Eq, Ne, Lt, Le, Gt, Ge,
    And, Or,
    Is, IsNot,
    In, NotIn,
    JsonExtract, JsonExtractDeep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LikeOp { Like, Glob, Regexp, Match }

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionArgs {
    Star,
    Distinct(Vec<Expr>),
    List(Vec<Expr>),
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InRhs {
    Subquery(Box<SelectStmt>),
    List(Vec<Expr>),
    Table { schema: Option<String>, name: String },
    TableFunction { schema: Option<String>, name: String, args: Vec<Expr> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaseArm { pub when: Expr, pub then: Expr }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaiseKind { Ignore, Rollback, Abort, Fail }

#[derive(Debug, Clone, PartialEq)]
pub struct WindowDef { pub name: String, pub spec: WindowSpec }

#[derive(Debug, Clone, PartialEq)]
pub struct WindowSpec {
    pub base: Option<String>,
    pub partition_by: Vec<Expr>,
    pub order_by: Vec<OrderingTerm>,
    pub frame: Option<WindowFrame>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowFrame {
    pub kind: FrameKind,
    pub start: FrameBound,
    pub end: Option<FrameBound>,
    pub exclude: Option<FrameExclude>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind { Range, Rows, Groups }

#[derive(Debug, Clone, PartialEq)]
pub enum FrameBound {
    UnboundedPreceding,
    Preceding(Box<Expr>),
    CurrentRow,
    Following(Box<Expr>),
    UnboundedFollowing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameExclude { NoOthers, CurrentRow, Group, Ties }

/// WITH (CTE) clause.
#[derive(Debug, Clone, PartialEq)]
pub struct WithClause {
    pub recursive: bool,
    pub ctes: Vec<CommonTableExpr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommonTableExpr {
    pub name: String,
    pub columns: Vec<String>,
    pub materialized: Option<bool>,
    pub select: Box<SelectStmt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictAction { Rollback, Abort, Fail, Ignore, Replace }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionKind { Deferred, Immediate, Exclusive }

#[derive(Debug, Clone, PartialEq)]
pub struct QualifiedTable {
    pub schema: Option<String>,
    pub name: String,
    pub alias: Option<String>,
    pub indexed: IndexedKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PragmaValue {
    Ident(String),
    Literal(LiteralValue),
}
