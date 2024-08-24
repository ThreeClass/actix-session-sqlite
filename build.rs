/**
	This is necessary for sqlx to work well in all circumstances, right now, it doesn't work well with different databases
	in different workspaces:

	See: https://github.com/launchbadge/sqlx/issues/267
	https://github.com/launchbadge/sqlx/issues/3099
*/

fn main() {

	let t = env!("CARGO_MANIFEST_DIR");
	println!("cargo:rustc-env=DATABASE_URL=sqlite:{}/session.db", t);
}