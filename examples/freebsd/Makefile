PORTNAME=	emfor
DISTVERSIONPREFIX=	v
DISTVERSION=	0.4.0
CATEGORIES=	net-mgmt

MAINTAINER=	ports@FreeBSD.org
COMMENT=	Health-check daemon that publishes healthy backends in a local store
WWW=		https://github.com/embark-systems/emfor

LICENSE=	BSD2CLAUSE
LICENSE_FILE=	${WRKSRC}/LICENSE

USES=		cargo
USE_GITHUB=	yes
GH_ACCOUNT=	embark-systems

# The repository root is a workspace: build and install only the daemon and
# emfor-check. EMFOR_PREFIX makes ${PREFIX}/etc/emfor/emfor.toml the
# default config path of emfor server, emfor check-config and emfor status.
CARGO_BUILD_ARGS=	--package emfor-daemon --package emfor-check
CARGO_INSTALL_PATH=	crates/emfor-daemon crates/emfor-check
CARGO_ENV+=	EMFOR_PREFIX=${PREFIX}

# Add both to ports/UIDs and ports/GIDs (or pick free ids) before building.
USERS=		emfor
GROUPS=		emfor

# files/emfor.in is the rc.d script; USE_RC_SUBR fills in %%PREFIX%%.
USE_RC_SUBR=	emfor

# The store goes in /var/db/emfor, where FreeBSD keeps daemon state. The
# daemon and the clients default to /var/lib/emfor/state.mdb, so the sample
# config sets [store] path, and applications set EMFOR_STORE (README.md).
PLIST_FILES=	bin/emfor \
		bin/emfor-check \
		"@sample etc/emfor/emfor.toml.sample" \
		"@dir(emfor,emfor,0755) /var/db/emfor"

# The sample config comes from the source's examples/, with the store path
# changed to /var/db/emfor.
post-install:
	${MKDIR} ${STAGEDIR}${PREFIX}/etc/emfor ${STAGEDIR}/var/db/emfor
	${SED} -e 's|/var/lib/emfor/|/var/db/emfor/|' \
		${WRKSRC}/examples/emfor.toml > ${WRKDIR}/emfor.toml.sample
	${INSTALL_DATA} ${WRKDIR}/emfor.toml.sample \
		${STAGEDIR}${PREFIX}/etc/emfor/emfor.toml.sample

.include <bsd.port.mk>
