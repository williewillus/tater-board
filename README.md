# Taterboard

A bot used to save messages blessed with tiny potatoes.

React to messages with the :tinypotato: emote (or any emote, it's customizeable!), and once it gets enough it will get saved to a channel of your choosing. Our grandchildren's children will truly enjoy all the wonderful hot takes, barely-cropped hentai, and poop jokes we will preserve for them.

Each server the bot is in is handled completely separately. Messages from one server will never get saved in another server, for example.

By default, the threshold for saving a message is 5 potatoes, but that's configurable. New potatoey medals are unlocked at 2x, 4x, 8x, and 16x the threshold.

## Setup Guide

1) Invite the bot to your server
2) If you have a role with administrator privileges, you can access the admin commands. Type in `potato set_pin_channel <channel_id>`, where `<channel_id>` is the ID of the channel you want the pinned messages to go to
3) ???
4) Profit

## Commands

Normal commands:

- `help`: Get this message.
- `receivers <page_number>`: See the most protatolific receivers of potatoes. `page_number` is optional.
- `givers <page_number>`: See the most protatolific givers of potatoes. `page_number` is optional.

Admin commands are only open to people with at least one role granting the Administrator privilege (or people with my user ID, cause I gotta test it somehow.)

- `set_pin_channel <channel_id>`: Set the channel that pinned messages to go, and adds it to the potato blacklist.
- `set_potato <emoji>`: Set the given emoji to be the operative one.
- `set_threshold <number>`: Set how many potatoes have to be on a message before it is pinned.
- `blacklist <channel_id>`: Make the channel no longer eligible for pinning messages, regardless of potato count.
- `unblacklist <channel_id>`: Unblacklist this channel so messages from it can be pinned again.
- `save`: Save this server's information to the server the bot is running on in case it goes down.

## Hosting the Bot Yourself

This repo should include everything you need to host the bot yourself. Just clone it and `cargo build` it.

The program expects you to put your bot's API key in the `TATERBOARD_TOKEN` environment variable. It also expects the second argument to be the path to a folder where it will save the `.json` files when `potato save` is run. Upon launching, it will read out all the files in that directory so it can restore from a previous point.

Each `.json` file is named `<guild_id>.json` where `<guild_id>` is the ID of the guild (aka "discord server") the data inside is associated with.
