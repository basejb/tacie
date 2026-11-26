# Connecting your tools

Tacie finds answers in the tools you connect. In this edition you connect each one with a personal token that you make
on that service. Open **Settings → Connections** and click **Connect** on a service. A window opens
with a link to the page where you make the token and a field where you paste it.

Tacie checks a pasted token with the service right away and keeps only a token the service accepts, in your Mac's
keychain. If the service cannot be reached, the window says so and offers **Check again**. Your data comes
straight from the service to your Mac, and Tacie only reads it. Tacie never refreshes a token: if the service stops
accepting it, the card shows **Reconnect** and you paste a new one.

## Why a token?

The official app connects with one button. That button signs you in through an app that Tacie registered with each
service, and this edition does not include those registrations. A personal token connects without one. What Tacie
can read depends on the token and on what your account can see, and you can delete the token on the service at any
time.

## GitHub

1. Click **Create a token on GitHub**. A fine-grained token page opens with the name and the
   permissions already filled in: Contents, Issues, Pull requests and Metadata, all read-only.
2. Under **Repository access**, choose the repositories Tacie may read.
3. Click **Generate token**, copy it (it starts with `github_pat_`) and paste it into the window.
4. Pick the repositories to search.

Repositories of an organization appear only when you make the token with that organization as the **Resource owner**.
If the organization requires approval for tokens, they appear after an admin approves it.

## Notion

1. Click **Create an integration in Notion**. On Notion's integrations page, create a
   **New integration** named Tacie, of the internal type, in the workspace you want to search.
2. On the **Configuration** tab, copy the token (it starts with `ntn_`) and paste it into the window.
3. On the integration's **Content access** tab, add the pages Tacie may read. Tacie reads the pages you add there,
   with their child pages, databases and content, and nothing else in the workspace.

## Linear

1. Click **Create an API key in Linear**. It opens **Settings → Security & access**.
2. Under **Personal API keys**, click **New API key**.
3. Copy the key (it starts with `lin_api_`) and paste it into the window. Tacie reads the issues and projects your
   account can see.

To disconnect, delete the key in the same place.

## Jira and Confluence

Jira and Confluence share one Atlassian API token.

1. Click **Create a token in Atlassian**. It opens your Atlassian account's API tokens page.
2. Click **Create API token**, name it Tacie and choose when it expires.
3. In the window, enter your site address (`myteam` or `myteam.atlassian.net`), the email you sign in to Atlassian
   with, and the token.

Tacie reads Jira and Confluence on that site with what your account can see. When the token expires, make a new one
and reconnect.

## Asana

1. Click **Create a token in Asana**. It opens **My apps** in the Asana developer console.
2. Under **Personal access tokens**, create a token named Tacie, copy it and paste it into the window.

Tacie reads the tasks and projects your account can see. To disconnect, delete the token under
**My apps → Personal access tokens**.

## Obsidian and Markdown folders

No token is needed. On the Obsidian card, choose your vault or any folder of Markdown files. Tacie reads the `.md`
files on your Mac.

## Not in this edition

Google Drive and Dropbox need an app registered with each of them, so this edition does not offer them.
