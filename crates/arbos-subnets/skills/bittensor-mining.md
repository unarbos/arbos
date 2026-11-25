How to go from "mine subnet N" to a running miner. Read `bittensor-basics` first for the rules about keys and spending.

1. **Read the subnet.** Open the skill `bittensor-sn<N>`. It names the repository and the commit it was written from. Clone that repository and read its miner guide yourself; the skill is a summary and may be behind.
2. **Check it is worth it** before any spending: the registration fee now (`btcli subnets list`, `btcli subnets show --netuid N`), the hardware the miner guide asks for and what that costs per day, and what the lowest-ranked miners currently earn. Tell the user the fee, the daily cost and that the result may be zero.
3. **Keys.** The user's coldkey stays on their device. Make a new hotkey for this miner (`btcli wallet new-hotkey`); only the hotkey goes to the machine that mines.
4. **Register** the hotkey on the subnet (`btcli subnets register --netuid N`). This spends the fee from the coldkey, so it is the user's to approve and to sign.
5. **Get a machine** that meets the guide's requirements. GPU and CPU can be rented from other subnets, which is usually cheaper than a cloud: Lium (subnet 51) rents GPU machines, Targon (subnet 4) and Chutes (subnet 64) run compute and model serving. Read their `bittensor-sn` skills for how to rent. Model inference the miner needs can go through the models Arbos already offers.
6. **Run the miner** exactly as the subnet's guide says, under a process manager so it restarts. Open only the ports the guide names.
7. **Watch it.** `btcli wallet overview` shows the miner's rank and earnings. Report earnings against what was spent on the fee and the machine. If it earns less than it costs for a few days, say so and propose stopping.

`btcli` changes between releases: run `btcli <command> --help` before trusting a flag written here or in an old guide.
