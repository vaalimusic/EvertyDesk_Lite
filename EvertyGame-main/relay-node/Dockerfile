FROM mcr.microsoft.com/dotnet/sdk:8.0 AS build
WORKDIR /src
COPY relay-node/Everty.RelayNode.csproj relay-node/
RUN dotnet restore relay-node/Everty.RelayNode.csproj
COPY relay-node/ relay-node/
RUN dotnet publish relay-node/Everty.RelayNode.csproj -c Release -o /app/publish --no-restore

FROM mcr.microsoft.com/dotnet/runtime:8.0
WORKDIR /app
ENV EVERTY_RELAY_CONTROL_PLANE=http://control-plane:5180
ENV EVERTY_RELAY_DISPLAY_NAME="Docker Relay"
ENV EVERTY_RELAY_REGION=local
ENV EVERTY_RELAY_PUBLIC_ADDRESS=127.0.0.1
ENV EVERTY_RELAY_UDP_PORT=6200
EXPOSE 6200/udp
COPY --from=build /app/publish .
ENTRYPOINT ["dotnet", "Everty.RelayNode.dll"]
