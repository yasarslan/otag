# Security Policy

## Supported versions

otag is pre-alpha. Only the `main` branch receives fixes.

## Reporting a vulnerability

Please report security issues privately, through GitHub's private vulnerability
reporting: open the repository's **Security** tab and choose **Report a
vulnerability**. Do not open a public issue or pull request for a security
problem.

Include what you did, what happened, and what you expected, along with the otag
commit and your operating system. A proof of concept helps.

You can expect an acknowledgement within a week. Once a fix is ready it is
released on `main` with a note in `CHANGELOG.md`, and you will be credited
unless you prefer otherwise.

## Scope

otag runs other programs on your machine, reads their local session files, and
listens on a Unix socket that is restricted to your own user account. It makes
no network connections and sends nothing anywhere.

Reports that are in scope include: a way for another local user to reach the
socket or the data behind it, credentials or secrets ending up in otag's logs or
state, otag writing outside its own directories without the confirmation it
promises, and anything that lets untrusted input run commands.

Out of scope: problems in the agents otag runs — report those to their own
projects.
