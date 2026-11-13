PORTNAME=	emfor
DISTVERSIONPREFIX=	v
DISTVERSION=	0.1.1
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
# default config path of emfor, emfor status and emfor --check-config.
CARGO_BUILD_ARGS=	--package emfor-daemon --package emfor-check
CARGO_INSTALL_PATH=	crates/emfor-daemon crates/emfor-check
CARGO_ENV+=	EMFOR_PREFIX=${PREFIX}

# Add both to ports/UIDs and ports/GIDs (or pick free ids) before building.
USERS=		emfor
GROUPS=		emfor

# The store stays at /var/lib/emfor/state.mdb, the default of the daemon and
# of every client (EMFOR_STORE), so applications need no setting.
PLIST_FILES=	bin/emfor \
		bin/emfor-check \
		etc/rc.d/emfor \
		"@sample etc/emfor/emfor.toml.sample" \
		"@dir(emfor,emfor,0755) /var/lib/emfor"

# The rc.d script and the config come from the source's examples/: run as
# the emfor user, and look in ${PREFIX} rather than /usr/local.
post-install:
	${MKDIR} ${STAGEDIR}${PREFIX}/etc/rc.d ${STAGEDIR}${PREFIX}/etc/emfor \
		${STAGEDIR}/var/lib/emfor
	${SED} -e 's|/usr/local|${PREFIX}|g' -e 's|emfor_user:=root|emfor_user:=emfor|' \
		-e 's|(default root)|(default emfor)|' \
		${WRKSRC}/examples/rc.d/emfor > ${STAGEDIR}${PREFIX}/etc/rc.d/emfor
	${CHMOD} 0555 ${STAGEDIR}${PREFIX}/etc/rc.d/emfor
	${INSTALL_DATA} ${WRKSRC}/examples/emfor.toml \
		${STAGEDIR}${PREFIX}/etc/emfor/emfor.toml.sample

.include "${.CURDIR}/Makefile.crates"
.include <bsd.port.mk>
