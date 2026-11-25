# Quickstart

## Send your first message

[Install kuku](install.md), open a terminal in your project directory, and run:

```bash
kuku
```

On a fresh data directory, choose a provider preset or add a custom connection in the setup flow. Supply credentials in the form you want to use and select one exact model ID. Choose **Save and continue** to save it as the default for new Sessions. Press `F1` during setup to open the configuration guide for your build. [Configuration](configuration.md)

In the composer, type `Inspect the files in this directory and tell me how to run its tests` and press Enter. Your first message creates a Session. The status line shows the workspace, Agent, and active model.

While the Agent is running, **Enter** sends a follow-up at the next safe model-request boundary and **Alt+Enter** queues it for the next Turn. Closing the terminal leaves accepted work running in the local service. [Terminal controls](../reference/cli.md#terminal-interface)

## Ask for help

After setting up a model, ask `How do I resume my last Session?` The Agent can look up the steps in the docs shipped with your running service. You can also press `F1` or type `/help` for shortcuts, or select `kuku-docs` from the `/` menu and type your question after the `/skill:kuku-docs` prefix. [Use Skills](../guides/use-skills.md#ask-about-kuku)

## Return to the same Session

From another terminal in the same workspace, run `kuku` and choose `/resume` to reopen the Session. [Manage Sessions](../guides/manage-sessions.md)

## Run from a script

Use `kuku run` from a terminal or script. The [headless walkthrough](../guides/run-a-task.md) covers setting up from a shell, sending a prompt from a file or standard input, and reading the result.
