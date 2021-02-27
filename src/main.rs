// Reference local files
mod post;
mod proxy;
mod search;
mod settings;
mod subreddit;
mod user;
mod utils;

// Import Crates
use clap::{App, Arg};
use proxy::handler;
use tide::{
	utils::{async_trait, After},
	Middleware, Next, Request, Response,
};
use utils::{error, redirect};

// Build middleware
struct HttpsRedirect<HttpsOnly>(HttpsOnly);
struct NormalizePath;

#[async_trait]
impl<State, HttpsOnly> Middleware<State> for HttpsRedirect<HttpsOnly>
where
	State: Clone + Send + Sync + 'static,
	HttpsOnly: Into<bool> + Copy + Send + Sync + 'static,
{
	async fn handle(&self, request: Request<State>, next: Next<'_, State>) -> tide::Result {
		let secure = request.url().scheme() == "https";

		if self.0.into() && !secure {
			let mut secured = request.url().to_owned();
			secured.set_scheme("https").unwrap_or_default();

			Ok(redirect(secured.to_string()))
		} else {
			Ok(next.run(request).await)
		}
	}
}

#[async_trait]
impl<State: Clone + Send + Sync + 'static> Middleware<State> for NormalizePath {
	async fn handle(&self, request: Request<State>, next: Next<'_, State>) -> tide::Result {
		let path = request.url().path();
		let query = request.url().query().unwrap_or_default();
		if path.ends_with('/') {
			Ok(next.run(request).await)
		} else {
			let normalized = if query != "" {
				format!("{}/?{}", path.replace("//", "/"), query)
			} else {
				format!("{}/", path.replace("//", "/"))
			};
			Ok(redirect(normalized))
		}
	}
}

// Create Services

// Required for the manifest to be valid
async fn pwa_logo(_req: Request<()>) -> tide::Result {
	Ok(Response::builder(200).content_type("image/png").body(include_bytes!("../static/logo.png").as_ref()).build())
}

// Required for iOS App Icons
async fn iphone_logo(_req: Request<()>) -> tide::Result {
	Ok(
		Response::builder(200)
			.content_type("image/png")
			.body(include_bytes!("../static/apple-touch-icon.png").as_ref())
			.build(),
	)
}

async fn favicon(_req: Request<()>) -> tide::Result {
	Ok(
		Response::builder(200)
			.content_type("image/vnd.microsoft.icon")
			.header("Cache-Control", "public, max-age=1209600, s-maxage=86400")
			.body(include_bytes!("../static/favicon.ico").as_ref())
			.build(),
	)
}

async fn resource(body: &str, content_type: &str, cache: bool) -> tide::Result {
	let mut res = Response::new(200);

	if cache {
		res.insert_header("Cache-Control", "public, max-age=1209600, s-maxage=86400");
	}

	res.set_content_type(content_type);
	res.set_body(body);

	Ok(res)
}

#[async_std::main]
async fn main() -> tide::Result<()> {
	let matches = App::new("Libreddit")
		.version(env!("CARGO_PKG_VERSION"))
		.about("Private front-end for Reddit written in Rust ")
		.arg(
			Arg::with_name("address")
				.short("a")
				.long("address")
				.value_name("ADDRESS")
				.help("Sets address to listen on")
				.default_value("0.0.0.0")
				.takes_value(true),
		)
		.arg(
			Arg::with_name("port")
				.short("p")
				.long("port")
				.value_name("PORT")
				.help("Port to listen on")
				.default_value("8080")
				.takes_value(true),
		)
		.arg(
			Arg::with_name("redirect-https")
				.short("r")
				.long("redirect-https")
				.help("Redirect all HTTP requests to HTTPS")
				.takes_value(false),
		)
		.get_matches();

	let address = matches.value_of("address").unwrap_or("0.0.0.0");
	let port = matches.value_of("port").unwrap_or("8080");
	let force_https = matches.is_present("redirect-https");

	let listener = format!("{}:{}", address, port);

	println!("Starting Libreddit...");

	// Start HTTP server
	let mut app = tide::new();

	// Redirect to HTTPS if "--redirect-https" enabled
	app.with(HttpsRedirect(force_https));

	// Append trailing slash and remove double slashes
	app.with(NormalizePath);

	// Apply default headers for security
	app.with(After(|mut res: Response| async move {
		res.insert_header("Referrer-Policy", "no-referrer");
		res.insert_header("X-Content-Type-Options", "nosniff");
		res.insert_header("X-Frame-Options", "DENY");
		res.insert_header(
			"Content-Security-Policy",
			"default-src 'none'; manifest-src 'self'; media-src 'self'; style-src 'self' 'unsafe-inline'; base-uri 'none'; img-src 'self' data:; form-action 'self'; frame-ancestors 'none';",
		);
		Ok(res)
	}));

	// Read static files
	app.at("/style.css/").get(|_| resource(include_str!("../static/style.css"), "text/css", false));
	app
		.at("/manifest.json/")
		.get(|_| resource(include_str!("../static/manifest.json"), "application/json", false));
	app.at("/robots.txt/").get(|_| resource("User-agent: *\nAllow: /", "text/plain", true));
	app.at("/favicon.ico/").get(favicon);
	app.at("/logo.png/").get(pwa_logo);
	app.at("/touch-icon-iphone.png/").get(iphone_logo);
	app.at("/apple-touch-icon.png/").get(iphone_logo);

	// Proxy media through Libreddit
	app
		.at("/vid/:id/:size/") /*      */
		.get(|req| handler(req, "https://v.redd.it/{}/DASH_{}", vec!["id", "size"]));
	app
		.at("/img/:id/") /*            */
		.get(|req| handler(req, "https://i.redd.it/{}", vec!["id"]));
	app
		.at("/thumb/:point/:id/") /*   */
		.get(|req| handler(req, "https://{}.thumbs.redditmedia.com/{}", vec!["point", "id"]));
	app
		.at("/emoji/:id/:name/") /*    */
		.get(|req| handler(req, "https://emoji.redditmedia.com/{}/{}", vec!["id", "name"]));
	app
		.at("/preview/:loc/:id/:query/")
		.get(|req| handler(req, "https://{}view.redd.it/{}?{}", vec!["loc", "id", "query"]));
	app
		.at("/style/*path/") /*        */
		.get(|req| handler(req, "https://styles.redditmedia.com/{}", vec!["path"]));
	app
		.at("/static/*path/") /*       */
		.get(|req| handler(req, "https://www.redditstatic.com/{}", vec!["path"]));

	// Browse user profile
	app.at("/u/:name/").get(user::profile);
	app.at("/u/:name/comments/:id/:title/").get(post::item);
	app.at("/u/:name/comments/:id/:title/:comment_id/").get(post::item);

	app.at("/user/:name/").get(user::profile);
	app.at("/user/:name/comments/:id/").get(post::item);
	app.at("/user/:name/comments/:id/:title/").get(post::item);
	app.at("/user/:name/comments/:id/:title/:comment_id/").get(post::item);

	// Configure settings
	app.at("/settings/").get(settings::get).post(settings::set);
	app.at("/settings/restore/").get(settings::restore);

	// Subreddit services
	app.at("/r/:sub/").get(subreddit::page);

	app.at("/r/:sub/subscribe/").post(subreddit::subscriptions);
	app.at("/r/:sub/unsubscribe/").post(subreddit::subscriptions);

	app.at("/r/:sub/comments/:id/").get(post::item);
	app.at("/r/:sub/comments/:id/:title/").get(post::item);
	app.at("/r/:sub/comments/:id/:title/:comment_id/").get(post::item);

	app.at("/r/:sub/search/").get(search::find);

	app.at("/r/:sub/wiki/").get(subreddit::wiki);
	app.at("/r/:sub/wiki/:page/").get(subreddit::wiki);
	app.at("/r/:sub/w/").get(subreddit::wiki);
	app.at("/r/:sub/w/:page/").get(subreddit::wiki);

	app.at("/r/:sub/:sort/").get(subreddit::page);

	// Front page
	app.at("/").get(subreddit::page);

	// View Reddit wiki
	app.at("/w/").get(subreddit::wiki);
	app.at("/w/:page/").get(subreddit::wiki);
	app.at("/wiki/").get(subreddit::wiki);
	app.at("/wiki/:page/").get(subreddit::wiki);

	// Search all of Reddit
	app.at("/search/").get(search::find);

	// Handle about pages
	app.at("/about/").get(|req| error(req, "About pages aren't here yet".to_string()));

	app.at("/:id/").get(|req: Request<()>| async {
		match req.param("id") {
			// Sort front page
			Ok("best") | Ok("hot") | Ok("new") | Ok("top") | Ok("rising") | Ok("controversial") => subreddit::page(req).await,
			// Short link for post
			Ok(id) if id.len() > 4 && id.len() < 7 => post::item(req).await,
			// Error message for unknown pages
			_ => error(req, "Nothing here".to_string()).await,
		}
	});

	// Default service in case no routes match
	app.at("*").get(|req| error(req, "Nothing here".to_string()));

	println!("Running Libreddit v{} on {}!", env!("CARGO_PKG_VERSION"), listener);

	app.listen(&listener).await?;

	Ok(())
}
