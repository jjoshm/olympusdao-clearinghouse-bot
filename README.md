# Clearinghouse MEV Keeper Bot for Olympusdao
![image](https://github.com/jjoshm/olympusdao-liquidation-bot/assets/39901876/7a2e6964-44a6-4c24-b7ae-7d195a41011e)
Bot earns rewards by executing transactions on the Olympusdao Clearinghouse Smart Contract.
You can find the current Clearinghouse Contract here: https://etherscan.io/address/0xE6343ad0675C9b8D3f32679ae6aDbA0766A2ab4c#code

---

Bot made around $1000 profit in 3 weeks and is now open sourced to enhance competition.

---

## Run using Docker
You need to update the env variables accordingly.

Make sure to use wss for the `RPC_PROVIDER_READ`.

```
docker run --name olympusdao-clearinghouse-bot --restart unless-stopped \
         -e PRIVATE_KEY=1234 \
         -e RPC_PROVIDER_READ=wss://eth-mainnet.g.alchemy.com/xxxxx \
         -e RPC_PROVIDER_SIGN=https://rpc.flashbots.net/fast \
         -e COOLER_FACTORY_ADDRESS=0x30Ce56e80aA96EbbA1E1a74bC5c0FEB5B0dB4216 \
         -e CLEARINGHOUSE_ADDRESS=0xE6343ad0675C9b8D3f32679ae6aDbA0766A2ab4c \
         -e MIN_PROFIT=100 \
         -e REWARD_PERIOD_TARGET=10 \
         ghcr.io/jjoshm/olympusdao-clearinghouse-bot:main
```


---

## TODO
- refactor
- add error handling / fix err after successful tx
  
---

Add me on Discord `ninjo.sh` https://discord.com/users/270271067459682306

---

Thanks to https://github.com/paradigmxyz/artemis
