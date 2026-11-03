//! Sea-ORM entities for the OLTP-owned tables.
//!
//! Cross-table `Related` impls to `entity::organizations` are intentionally
//! omitted — they would be dead code. The FK constraint still exists at the
//! database level (see [`crate::migration`]), which is what actually enforces
//! the relationship.

pub mod branch_roles;
pub mod branches;
pub mod env_branch_roles;
pub mod env_branches;
pub mod roles;
pub mod tenants;

pub use branch_roles::Entity as BranchRoles;
pub use branches::Entity as Branches;
pub use env_branch_roles::Entity as EnvBranchRoles;
pub use env_branches::Entity as EnvBranches;
pub use roles::Entity as Roles;
pub use tenants::Entity as Tenants;
